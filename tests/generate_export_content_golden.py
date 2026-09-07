"""仅执行白名单 AST 纯函数；不导入旧服务，不读取聊天、配置或密钥。"""

import ast
import argparse
import base64
import json
from pathlib import Path
import re
from types import SimpleNamespace
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape


ROOT = Path(__file__).resolve().parents[1]


def extract(path, functions, constants, scope):
    tree = ast.parse(path.read_text(encoding="utf-8"))
    selected = []
    found = set()
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in functions:
            selected.append(node)
            found.add(node.name)
        elif isinstance(node, ast.Assign):
            names = {target.id for target in node.targets if isinstance(target, ast.Name)}
            if names & constants:
                selected.append(node)
                found.update(names & constants)
    assert found == functions | constants, (path, (functions | constants) - found)
    # 只编译选中的定义与常量；绝不执行模块级 import、初始化或数据库连接。
    exec(compile(ast.Module(body=selected, type_ignores=[]), str(path), "exec"), scope)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="只读核验现有 golden 与旧纯函数输出一致")
    args = parser.parse_args()
    scope = {"re": re, "ET": ET, "base64": base64}
    functions = {
        "_split_msg_type", "_collapse_text", "_parse_xml_root", "_parse_int",
        "_parse_app_message_outer", "_format_app_message_text", "_extract_transfer_info",
        "_format_transfer_message_text", "_format_voip_message_text",
        "_format_record_message_text", "_format_record_dataitem",
        "_format_redpacket_message_text", "_format_finder_message_text",
        "_extract_refer_info", "_summarize_refer_content", "_format_refer_message_text",
        "_resolve_quote_sender_label", "_display_name_for_username",
    }
    extract(ROOT / "vendor/wechat-decrypt/mcp_server.py", functions, {
        "_XML_PARSE_MAX_LEN", "_XML_UNSAFE_RE", "_RECORD_XML_PARSE_MAX_LEN",
        "_TRANSFER_PAYSUBTYPE_LABEL",
        "_RECORD_MAX_ITEMS", "_RECORD_MAX_LINE_LEN", "_RECORD_DATATYPE_LABEL",
        "_REFER_INNER_TYPE_LABEL", "_INNER_APPMSG_TYPE_LABEL", "_REDPACKET_AMOUNT_SCENES",
        "_REDPACKET_AMOUNT_RE", "_REDPACKET_SENDER_RE",
    }, scope)
    server = SimpleNamespace(**{name: scope[name] for name in functions})
    # 纯函数入口只接收已经解压的正文，联系人上下文固定为空，群前缀由调用方处理。
    server._decompress_content = lambda content, ct: content
    server._parse_message_content = lambda content, kind, group: ("", content)
    server.get_contact_names = lambda: {}
    scope["mcp_server"] = server
    extract(ROOT / "vendor/wechat-decrypt/chat_export_helpers.py", {
        "_extract_content", "_decode_sticker_desc", "_format_sticker_message",
        "_format_system_message", "_format_video_message", "_extract_transfer_extras",
    }, set(), scope)
    cases = []

    def add(name, kind, content, context=None):
        ctx = context or {}
        scope["_get_self_username"] = lambda: ctx.get("self_username", "")
        server.get_contact_names = lambda: ctx.get("names", {})
        rendered, extras = scope["_extract_content"](
            1, kind, content, 0, ctx.get("chat_username", ""), ctx.get("chat_display_name", ""))
        case = {"name": name, "type": kind, "input": content,
                "expected": {"content": rendered, "extras": extras or {}}}
        if context is not None:
            assert bool(ctx.get("is_group")) == ctx.get("chat_username", "").endswith("@chatroom")
            case["context"] = context
        cases.append(case)

    def app(name, body, subtype=0):
        add(name, (subtype << 32) | 49, f"<msg><appmsg>{body}</appmsg></msg>")

    for kind in [1, 3, 34, 42, 43, 47, 48, 49, 50, 10000, 10002, 99, -1]:
        for index, content in enumerate([None, "", "合成正文\n  保留空白", "<msg/>", "<msg>"]):
            add(f"base-{kind}-{index}", kind, content)
        add(f"packed-{kind}", (7 << 32) | kind if kind >= 0 else kind, "合成正文")
    for kind in [3, 34, 42, 48]:
        add(f"omit-readable-{kind}", kind, '<msg nickname="合成"><location label="合成地址"/><voicemsg voicelength="1500"/></msg>')
    for index, body in enumerate(["<sysmsg><content>  系统\n 正文  </content></sysmsg>",
                                  "<sysmsg><content>   </content></sysmsg>",
                                  "<sysmsg><content/></sysmsg>", "<sysmsg/>",
                                  "<sysmsg><content>前<b/>后</content></sysmsg>",
                                  "<!DOCTYPE sysmsg><sysmsg><content>不解析</content></sysmsg>"]):
        add(f"system-{index}", 10000, body)
    for length in ["5", "0", "unknown", " 10 ", ""]:
        add(f"video-{length}", 43, f'<msg><videomsg playlength="{length}"/></msg>')
    for status in ["Canceled", "Line busy", "Duration: 01:23", "Duration: ", "未知状态", ""]:
        add(f"call-{status}", 50, f"<voip><msg>{status}</msg></voip>")
    for name, raw in [
        ("chinese", b"\x0a\x07default\x12\x06" + "你好".encode()),
        ("truncated-length", b"default\x12\x7fshort"),
        ("single-byte-not-varint", b"default\x12\x80\x01text"),
        ("empty", b"default\x12\x00"), ("bad-utf8", b"default\x12\x01\xff"),
        ("no-marker", b"english"), ("missing-length", b"default\x12"),
        ("first-marker-wins", b"default?default\x12\x01x"),
    ]:
        desc = base64.b64encode(raw).decode()
        add(f"sticker-{name}", 47, f'<msg><emoji desc="{desc}"/></msg>')
    desc = base64.b64encode(b"default\x12\x01x").decode()
    for index, value in enumerate([desc, "! " + desc + "!", desc + "ignored", desc.rstrip("="), "???", "中", "Z==="]):
        add(f"sticker-base64-{index}", 47, f'<msg><emoji desc="{value}"/></msg>')
    for kind in [5, 6, 33, 36, 44, 0, 999]:
        for title in ["", "  合成\n 标题  "]:
            app(f"app-{kind}-{title}", f"<type>{kind}</type><title>{title}</title>")
    for value in ["", "bad", "+0006", "0_006", "-6", "999999999999999999999999999"]:
        app(f"app-subtype-{value}", f"<type>{value}</type><title>标题</title>", 5)
    app("app-cdata", "<type>5</type><title><![CDATA[a < b & c]]></title>")
    app("app-direct-text-only", "<type>6</type><title>前<b/>后</title>")
    for index, body in enumerate(["", "<wcpayinfo/>",
            "<wcpayinfo><paysubtype>1</paysubtype><feedesc>¥0.01</feedesc><pay_memo>  合成\n 备注 </pay_memo></wcpayinfo>",
            "<wcpayinfo><paysubtype>99</paysubtype><feeDesc>USD 2.00</feeDesc><transferId>synthetic-transfer</transferId><beginTransferTime>+001_234</beginTransferTime><invalidTime>0</invalidTime></wcpayinfo>"]):
        app(f"transfer-{index}", "<type>2000</type><title>合成转账</title>" + body)
    app("transfer-packed-no-extras", "<title>合成</title><wcpayinfo><paysubtype>1</paysubtype></wcpayinfo>", 2000)
    app("transfer-underscored-type", "<type>+02_000</type><wcpayinfo><paysubtype>3</paysubtype><feedesc>¥1.00</feedesc></wcpayinfo>")
    for index, content in enumerate(["<appmsg><type>6</type></appmsg>",
            "<!DOCTYPE msg><msg><appmsg><type>6</type></appmsg></msg>",
            "<msg><appmsg><type>6</type></msg>",
            '<msg xmlns="urn:synthetic"><appmsg><type>6</type></appmsg></msg>']):
        add(f"app-xml-{index}", 49, content)
    for size in [20_000, 20_001]:
        prefix, suffix = "<msg><appmsg><type>6</type><unused>", "</unused></appmsg></msg>"
        add(f"xml-limit-{size}", 49, prefix + "中" * (size - len(prefix) - len(suffix)) + suffix)
    # 原来四个不支持分支现在全部执行旧纯函数生成真实差分预期。
    for kind in [19, 57, 51, 2001]:
        app(f"expanded-empty-{kind}", f"<type>{kind}</type>")
        app(f"expanded-title-{kind}", f"<type>{kind}</type><title>  标题\n 内容 </title>")
        app(f"expanded-packed-{kind}", "<title>高位子类型</title>", kind)

    for index, body in enumerate([
        "<finderFeed><nickname> 合成\n 作者 </nickname><desc> 合成描述 </desc></finderFeed>",
        "<finderFeed><nickname>作者</nickname></finderFeed>",
        "<finderFeed><desc>没有作者则不采用描述</desc></finderFeed>",
        "<finderFeed><nickname>作者</nickname><desc>" + "中文" * 41 + "</desc></finderFeed>",
        "<wrapper><finderFeed><nickname>非直接子节点</nickname></finderFeed></wrapper>",
    ]):
        app(f"finder-{index}", "<type>51</type><title>回退标题</title>" + body)

    for index, (scene, greeting, desc, url) in enumerate([
        ("", "", "", ""), ("微信红包", "恭喜发财", "88.00元", "wxpay://synthetic?sendusername=fake_sender&amp;a=1"),
        ("群收款", " 聚餐\n 费用 ", "每人12.50 元", "sendusername=fake%20sender"),
        ("活动账单", "活动", "人均 ０.５ 元", "sendusername=first&amp;sendusername=second"),
        ("群收款", "", "没有金额", "sendusername="),
        ("群收款", "", "-5元，10元", "prefixsendusername=synthetic"),
    ]):
        app(f"redpacket-{index}", f"<type>2001</type><wcpayinfo><scenetext>{scene}</scenetext><sendertitle>{greeting}</sendertitle><senderdes>{desc}</senderdes><nativeurl>{url}</nativeurl></wcpayinfo>")

    def quote(name, kind="1", content="被引用正文", user="", display="", context=None):
        xml = f"<msg><appmsg><type>57</type><title> 回复\n 正文 </title><refermsg><type>{kind}</type><fromusr>{user}</fromusr><displayname>{display}</displayname><content>{escape(content)}</content></refermsg></appmsg></msg>"
        add(name, 49, xml, context)

    for kind in ["1", "3", "34", "42", "43", "47", "48", "49", "50", "99", ""]:
        quote(f"refer-type-{kind}", kind, "<msg><synthetic-attachment/></msg>")
        quote(f"refer-empty-{kind}", kind, "")
    quote("refer-text-160", content="中" * 160)
    quote("refer-text-161", content="中" * 161)
    quote("refer-whitespace", content=" \n  \t ")
    for inner_type in ["5", "6", "8", "19", "33", "36", "44", "51", "57", "2000", "2001", "999", ""]:
        quote(f"refer-inner-{inner_type}", "49", f"<msg><appmsg><type>{inner_type}</type><title> 嵌套\n 标题 </title></appmsg></msg>")
    for index, content in enumerate(["<msg>", "<msg/>", "<appmsg><type>6</type></appmsg>",
                                      "<!DOCTYPE msg><msg><appmsg/></msg>",
                                      '<msg xmlns="urn:synthetic"><appmsg/></msg>']):
        quote(f"refer-inner-invalid-{index}", "49", content)
    for group in [False, True]:
        context = {"is_group": group, "chat_username": "room@chatroom" if group else "peer",
                   "chat_display_name": "聊天备注", "self_username": "self",
                   "names": {"self": "我的昵称", "peer": "联系人备注", "empty": ""}}
        for index, (user, display) in enumerate([
            ("self", "旧昵称"), ("peer", "XML名称"), ("unknown", "XML名称"),
            ("unknown", ""), ("", "我的昵称"), ("", "聊天备注"), ("", "XML名称"),
            ("", ""), ("empty", "XML名称"), (context["chat_username"], "聊天XML名称"),
        ]):
            quote(f"refer-context-{group}-{index}", user=user, display=display, context=context)
    quote("refer-no-context-display", user="unknown", display="XML显示名")
    quote("refer-no-context-user", user="unknown")

    def record_case(name, inner, title="外层标题"):
        app(name, f"<type>19</type><title>{title}</title><recorditem>{escape(inner)}</recorditem>")

    for index, inner in enumerate(["", " ", "<recordinfo>", "<recordinfo/>",
            "<recordinfo><isChatRoom>1</isChatRoom></recordinfo>",
            "<!DOCTYPE recordinfo><recordinfo/>",
            "<recordinfo><title> 内层\n 标题 </title><datalist/></recordinfo>",
            "<recordinfo><wrapper><datalist><dataitem datatype='1'/></datalist></wrapper></recordinfo>"]):
        record_case(f"record-empty-{index}", inner)
    kinds = ["1", "2", "3", "4", "5", "6", "7", "8", "17", "19", "22", "23", "29", "36", "37", "99", ""]
    for populated in [False, True]:
        items = []
        for kind in kinds:
            body = "<sourcename> 合成\n 发送者 </sourcename><sourcetime>2026-01-01 00:00</sourcetime><datatitle>标题</datatitle><datadesc>描述</datadesc><appbranditem><sourcedisplayname>应用</sourcedisplayname></appbranditem><finderFeed><desc>视频描述</desc></finderFeed>" if populated else ""
            items.append(f"<dataitem datatype='{kind}'>{body}</dataitem>")
        record_case(f"record-types-{populated}", "<recordinfo><title>内层标题</title><isChatRoom>1</isChatRoom><datalist>" + "".join(items) + "</datalist></recordinfo>")
    for index, (kind, body) in enumerate([
        ("19", "<appbranditem><sourcedisplayname>应用名称</sourcedisplayname></appbranditem>"),
        ("29", "<datadesc>仅歌手不展示</datadesc>"), ("29", "<datatitle>歌名</datatitle>"),
        ("99", "<datatitle>未知标题</datatitle>"),
        ("22", "<finderFeed><desc>" + "中" * 81 + "</desc></finderFeed>"),
        ("1", "<datadesc>" + "中" * 200 + "</datadesc>"),
        ("1", "<datadesc>" + "中" * 201 + "</datadesc>"),
    ]):
        record_case(f"record-special-{index}", f"<recordinfo><datalist><dataitem datatype='{kind}'>{body}</dataitem></datalist></recordinfo>")
    for count in [50, 51]:
        record_case(f"record-count-{count}", "<recordinfo><datalist>" + "<dataitem datatype='1'><datadesc>正文</datadesc></dataitem>" * count + "</datalist></recordinfo>")
    for size in [20_001, 500_000, 500_001]:
        prefix, suffix = "<msg><appmsg><type>19</type><unused>", "</unused></appmsg></msg>"
        add(f"record-outer-limit-{size}", 49, prefix + "x" * (size - len(prefix) - len(suffix)) + suffix)
    prefix, suffix = "<msg><appmsg><type> 19 </type><unused>", "</unused></appmsg></msg>"
    add("record-large-requires-exact-marker", 49, prefix + "x" * 20_000 + suffix)
    target = ROOT / "tests/fixtures/export-content-golden.json"
    serialized = json.dumps(cases, ensure_ascii=False, indent=2) + "\n"
    if args.check:
        if target.read_text(encoding="utf-8") != serialized:
            raise SystemExit(f"Golden differs from legacy pure functions: {target}")
    else:
        target.write_text(serialized, encoding="utf-8")
    action = "Verified" if args.check else "Generated"
    print(f"{action} {len(cases)} synthetic differential cases: {target}")


if __name__ == "__main__":
    main()
