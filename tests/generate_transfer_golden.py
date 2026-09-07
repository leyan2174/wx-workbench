"""从旧实现的纯函数生成转账对照样本，不导入 MCP 服务或读取用户数据。"""

import ast
import datetime
import json
from pathlib import Path
import re
import xml.etree.ElementTree as ET
from types import SimpleNamespace


def main():
    root = Path(__file__).resolve().parents[1]
    tree = ast.parse((root / 'vendor/wechat-decrypt/mcp_server.py').read_text(encoding='utf-8'))
    selected = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in {'_collapse_text', '_parse_int', '_extract_transfer_info', '_format_transfer_message_text'}:
            selected.append(node)
        elif isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == '_TRANSFER_PAYSUBTYPE_LABEL' for t in node.targets):
            selected.append(node)
        elif isinstance(node, ast.FunctionDef) and node.name == 'decode_transfer':
            # 只复用已定位消息后的展示逻辑，不执行原函数的配置或数据库查询。
            start = next(i for i, child in enumerate(node.body) if isinstance(child, ast.FunctionDef) and child.name == '_fmt_ts')
            render = ast.parse('def render(appmsg, info):\n    pass').body[0]
            render.body = node.body[start:]
            selected.append(render)
    scope = {'re': re, 'datetime': datetime.datetime}
    exec(compile(ast.fix_missing_locations(ast.Module(body=selected, type_ignores=[])), '<legacy-transfer-pure>', 'exec'), scope)
    cases = []
    helper_tree = ast.parse((root / 'vendor/wechat-decrypt/chat_export_helpers.py').read_text(encoding='utf-8'))
    helper = next(node for node in helper_tree.body if isinstance(node, ast.FunctionDef) and node.name == '_extract_transfer_extras')
    scope['mcp_server'] = SimpleNamespace(_parse_app_message_outer=ET.fromstring, _parse_int=scope['_parse_int'], _collapse_text=scope['_collapse_text'], _extract_transfer_info=scope['_extract_transfer_info'])
    exec(compile(ast.Module(body=[helper], type_ignores=[]), '<legacy-transfer-export>', 'exec'), scope)

    def add(name, body, title='微信转账', compare_render=True):
        xml = f'<msg><appmsg><type>2000</type><title>{title}</title>{body}</appmsg></msg>'
        appmsg = ET.fromstring(xml).find('appmsg')
        info = scope['_extract_transfer_info'](appmsg)
        cases.append({'name': name, 'xml': xml, 'info': info, 'extras': scope['_extract_transfer_extras'](xml),
                      'summary': scope['_format_transfer_message_text'](appmsg, scope['_collapse_text'](appmsg.findtext('title'))),
                      'render': scope['render'](appmsg, info) if info is not None and compare_render else None})

    for subtype in ['1', '3', '4', '5', '7', '8', '99', '']:
        add(f'status-{subtype or "empty"}', f'<wcpayinfo><paysubtype>{subtype}</paysubtype><feedesc>¥0.01</feedesc><pay_memo>  早饭\n 报销 </pay_memo></wcpayinfo>')
    add('aliases', '<des> 描述\n 内容 </des><wcpayinfo><feeDesc>USD 1,234.50</feeDesc><paymemo>test memo</paymemo><transcationId>fake-pay-id</transcationId><transferId>fake-transfer-id</transferId><payMsgId>fake-message-id</payMsgId><payerUsername>fake-payer</payerUsername><receiverUsername>fake-receiver</receiverUsername><beginTransferTime>0</beginTransferTime><invalidTime>0</invalidTime><effectiveDate>tomorrow</effectiveDate></wcpayinfo>')
    add('alias-fallback', '<wcpayinfo><feedesc> </feedesc><feeDesc>¥2.00</feeDesc></wcpayinfo>')
    add('empty-info', '<wcpayinfo/>', '')
    add('missing-info', '', '仍保留的标题')
    add('xml-text', '<wcpayinfo><pay_memo><![CDATA[a < b & c]]></pay_memo></wcpayinfo>')
    add('huge-time', '<wcpayinfo><begintransfertime>999999999999999999999</begintransfertime><invalidtime>not-a-time</invalidtime></wcpayinfo>')
    add('out-of-datetime-range', '<wcpayinfo><begintransfertime>1000000000000</begintransfertime><invalidtime>+</invalidtime></wcpayinfo>')
    # 有效时间在本地时区显示；固定样本只比较其原始字段，不固定测试机器时区。
    add('valid-time-fields', '<wcpayinfo><begintransfertime>1700000000</begintransfertime><invalidtime>1700086400</invalidtime></wcpayinfo>', compare_render=False)
    output = root / 'tests/fixtures/transfer-golden.json'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(cases, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f'Generated {len(cases)} legacy transfer fixtures: {output}')


if __name__ == '__main__':
    main()
