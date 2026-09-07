-- 纯合成快照，不需要密钥、微信进程或后台服务。
CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);
CREATE TABLE SnsMessage_tmp3(feed_id INTEGER, create_time INTEGER, type INTEGER,
    from_username TEXT, from_nickname TEXT, to_username TEXT, to_nickname TEXT,
    content TEXT, del_status INTEGER);
