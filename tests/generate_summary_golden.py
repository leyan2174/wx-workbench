"""仅提取旧解析器的纯函数，生成无私人数据的摘要回归样本。"""
import ast
import json
import re
import xml.etree.ElementTree as ET
from types import SimpleNamespace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
source = ast.parse((ROOT / "vendor/wechat-decrypt/mcp_server.py").read_text(encoding="utf-8"))
names = {
    "_parse_xml_root", "_parse_int", "_collapse_text", "_format_voice_text",
    "_format_namecard_text", "_extract_location_info",
    "_is_location_poiname_placeholder", "_format_location_text",
    "_format_voip_message_text",
}
constants = {"_XML_PARSE_MAX_LEN", "_XML_UNSAFE_RE", "_LOCATION_TEXT_FIELDS"}
nodes = [node for node in source.body if
         isinstance(node, ast.FunctionDef) and node.name in names or
         isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in constants for t in node.targets)]
scope = {"re": re, "ET": ET}
exec(compile(ast.Module(body=nodes, type_ignores=[]), "legacy-pure-summary", "exec"), scope)
cases = []
helpers = ast.parse((ROOT / "vendor/wechat-decrypt/chat_export_helpers.py").read_text(encoding="utf-8"))
video = next(node for node in helpers.body if isinstance(node, ast.FunctionDef) and node.name == "_format_video_message")
scope["mcp_server"] = SimpleNamespace(_parse_xml_root=scope["_parse_xml_root"])
exec(compile(ast.Module(body=[video], type_ignores=[]), "legacy-video-summary", "exec"), scope)
for xml in ["", "<msg/>", "<msg>", "<videomsg playlength='5'/>"] + [f"<msg><videomsg playlength='{value}'/></msg>" for value in ["5", "0", "", "1.5", "unknown", " 10 "]]:
    cases.append({"kind": "video", "xml": xml, "expected": scope["_format_video_message"](xml)})
voip_cases = ["Canceled", "Line busy", "Already answered elsewhere", "Declined on other device", "Call canceled by caller", "Call not answered", "Call wasn't answered", "Duration: 01:23", "Duration: ", "通话时长 00:42", "未知状态", ""]
for xml in [f"<voip><msg>{status}</msg></voip>" for status in voip_cases] + ["<voip/>", "<voip>", "plain", "<!DOCTYPE voip><voip><msg>Canceled</msg></voip>", "<voip><msg><nested/>Canceled</msg></voip>"]:
    cases.append({"kind": "voip", "xml": xml, "expected": scope["_format_voip_message_text"](xml)})
for kind, xmls in {
    "voice": [f"<msg><voicemsg voicelength='{v}'/></msg>" for v in ["3300", "800", "62000", "0", "-1", "abc", " 1000 ", "1001", ""]],
    "namecard": ["<msg nickname='示例' username='gh_demo' certinfo='简介&#10; 第二行'/>", "<msg username='wxid_demo'/>", "<msg nickname='示例' antispamticket='synthetic'/>", "<msg/>"] ,
    "location": ["<msg><location/></msg>", "<msg><location poiname='[位置]' label='示例路'/></msg>", "<msg><location poiname='公园' label='示例路' poiCategoryTips='休闲:公园'/></msg>", "<msg><location poiname='公园' label='公园'/></msg>", "<msg><location poiCategoryTips='休闲:公园'/></msg>"],
}.items():
    for xml in xmls + ["", "<msg>", "<msg/>"]:
        cases.append({"kind": kind, "xml": xml, "expected": scope[f"_format_{kind}_text"](xml)})
target = ROOT / "tests/fixtures/summary-golden.json"
target.write_text(json.dumps(cases, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"Generated {len(cases)} synthetic legacy summary cases: {target}")

# 只执行详细工具从 lines 初始化开始的渲染部分，不引入数据库或 MCP 服务。
decode = next(node for node in source.body if isinstance(node, ast.FunctionDef) and node.name == "decode_location")
start = next(i for i, node in enumerate(decode.body) if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "lines" for t in node.targets))
render = ast.parse("def render(info):\n    pass\n").body[0]
render.body = decode.body[start:]
exec(compile(ast.fix_missing_locations(ast.Module(body=[render], type_ignores=[])), "legacy-location-render", "exec"), scope)
xmls = [case["xml"] for case in cases if case["kind"] == "location"]
attrs = " ".join(f"{name}='value{i}'" for i, name in enumerate(scope["_LOCATION_TEXT_FIELDS"]))
xmls += [f"<msg><location {attrs} x='31.2' y='121.5'/></msg>", "<msg><location x='-1.23456789' y='0'/></msg>", "<msg><location x='invalid' y='121.5'/></msg>"]
details = []
for xml in xmls:
    info = scope["_extract_location_info"](xml)
    details.append({"xml": xml, "info": info, "text": scope["render"](info) if info is not None else None})
target = ROOT / "tests/fixtures/location-golden.json"
target.write_text(json.dumps(details, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
print(f"Generated {len(details)} synthetic legacy location cases: {target}")
