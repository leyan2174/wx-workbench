"""企业微信查询合成回归：只导入旧导出脚本，以显式合成路径替代配置入口。"""
from contextlib import closing
import hashlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile

OUT = Path(__file__).resolve().parent
ROOT = OUT.parents[3]
SELF = 10000000001
PEER = 10000000002
SINGLE = f"S:{SELF}_{PEER}"


def create(path, script, inserts):
    with closing(sqlite3.connect(path)) as db:
        db.executescript(script)
        for sql, rows in inserts:
            db.executemany(sql, rows)
        db.commit()
        assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]


def main():
    source = ROOT / "vendor/wechat-decrypt/export_wxwork_messages.py"
    spec = importlib.util.spec_from_file_location("enterprise_export_oracle", source)
    legacy = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(legacy)
    with tempfile.TemporaryDirectory(prefix="enterprise-query-synthetic-") as directory:
        directory = Path(directory)
        create(directory / "user.db", """
            CREATE TABLE user_table(id INTEGER,name TEXT,real_name TEXT,account TEXT,external_corp_name TEXT,external_job TEXT);
            CREATE TABLE external_user_relation_v3(user_id INTEGER,remarks TEXT,real_remarks TEXT,corp_remark TEXT);
        """, [
            ("INSERT INTO user_table VALUES(?,?,?,?,?,?)", [
                (SELF,"本人","合成本人","me","",None),
                (PEER,"普通名","真实名","peer","合成公司","工程师"),
                (9007199254740993,"大整数联系人","","","",None),
                (33,"","","","仅公司",None),
            ]),
            ("INSERT INTO external_user_relation_v3 VALUES(?,?,?,?)", [(PEER,"备注","最终备注","公司备注")]),
        ])
        create(directory / "session.db", """
            CREATE TABLE conversation_table(id TEXT,name TEXT,roomname_remark TEXT,last_message_time INTEGER,last_message_id INTEGER,con_numeric_id INTEGER);
            CREATE TABLE conversation_user_table(conversation_id TEXT,user_id INTEGER,nick_name TEXT);
            CREATE TABLE conversation_member_nickname_table(room_id INTEGER,userid INTEGER,nickname TEXT);
        """, [
            ("INSERT INTO conversation_table VALUES(?,?,?,?,?,?)", [
                ("R:team","群原名","<团队 & 合成>",1700000003,6,88),
                (SINGLE,"","",1700000002,3,89),
                ("R:empty","没有消息","",0,0,90),
            ]),
            ("INSERT INTO conversation_user_table VALUES(?,?,?)", [("R:team",PEER,"群昵称一")]),
            ("INSERT INTO conversation_member_nickname_table VALUES(?,?,?)", [(88,PEER,"群昵称二"),(999,PEER,"不应出现")]),
        ])
        tables = legacy._MESSAGE_TABLES
        schema = "\n".join(f'''CREATE TABLE "{table}"(message_id INTEGER,server_id INTEGER,sequence INTEGER,sender_id INTEGER,conversation_id TEXT,content_type INTEGER,send_time INTEGER,flag INTEGER,content,extra_content,local_extra_content);''' for table in tables)
        segment = "协议文本".encode()
        proto = bytes([10,len(segment)]) + segment
        rows = [
            (1,101,1,SELF,"R:team",2,1700000000,0,'=1+2\n第二行,"quoted"',None,None),
            (2,102,2,PEER,"R:team",2,1700000001,1,proto,None,None),
            (3,103,3,PEER,SINGLE,4,1700000002,0,None,"图片说明",None),
            (4,104,4,0,"O:unlisted",999,1700000003,0,b"\xff",None,None),
        ]
        create(directory / "message.db", schema, [
            (f'INSERT INTO "{tables[0]}" VALUES(?,?,?,?,?,?,?,?,?,?,?)', rows),
            (f'INSERT INTO "{tables[1]}" VALUES(?,?,?,?,?,?,?,?,?,?,?)', [
                (1,101,1,PEER,"R:team",7,1700000000,0,"重复行应丢弃",None,None),
                (5,105,5,PEER,"R:team",7,1700000004,0,None,None,"语音说明"),
                (6,106,6,PEER,"R:team",2,1700000005,0,"中文回退".encode("gbk"),None,None),
            ]),
            (f'INSERT INTO "{tables[2]}" VALUES(?,?,?,?,?,?,?,?,?,?,?)', [
                (7,107,7,9007199254740993,"M:9007199254740993",2,1700000006000,0,"<script>alert(1)</script>",None,None),
            ]),
        ])
        # discover_conversations 原本读取真实配置；替换后仅能取得合成目录和显式本人 ID。
        legacy._load_config = lambda: {"decrypted_dir": str(directory), "self_id": SELF}
        users = legacy._load_user_map(str(directory))
        members = legacy._load_group_member_names(str(directory))
        conversations = legacy.discover_conversations(str(directory))
        conversations.sort(key=lambda c: (-c["last_time"], -c["message_count"], c["conversation_id"]))
        conv_map = {c["conversation_id"]: c for c in conversations}
        seen, messages = set(), []
        for row in legacy._iter_message_rows(str(directory / "message.db")):
            key = (row["conversation_id"],row["message_id"],row["server_id"],row["sequence"])
            if key not in seen:
                seen.add(key)
                messages.append(legacy._build_message(row,conv_map,users,members,SELF))
        messages.sort(key=lambda m:(m["send_time"],m["sequence"],m["message_id"],m["conversation_id"],m["source_table"]))
        for message in messages:
            message.pop("time")  # 本地时区显示另测，语义 golden 不依赖生成机器时区。
        decode_cases = [b"",b"hello\t world",proto,b"\x0a\xff",b"\xff"*11,"中文回退".encode("gbk"),"你好世界".encode("utf-16le"),b"\x12\x02\x08\x01",b"\x80"*12]
        fixture = {
            "synthetic_only": True, "self_id": SELF,
            "oracle": str(source.relative_to(ROOT)), "oracle_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            "contacts": [{"id": k,"display_name":v} for k,v in sorted(users.items())],
            "conversations": conversations, "messages": messages,
            "decode_cases": [{"hex":data.hex(),"text":legacy.decode_content(data)} for data in decode_cases],
            "sha256": {},
        }
        for name in ["user.db","session.db","message.db"]:
            data = (directory / name).read_bytes()
            (OUT / name).write_bytes(data)
            fixture["sha256"][name] = hashlib.sha256(data).hexdigest()
        (OUT / "golden.json").write_text(json.dumps(fixture,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    print(json.dumps({"synthetic_only":True,"contacts":len(users),"conversations":len(conversations),"raw_messages":8,"deduplicated_messages":len(messages),"decode_cases":len(decode_cases),"sha256":fixture["sha256"]},ensure_ascii=False,indent=2))


if __name__ == "__main__":
    main()
