use crate::support::{
    encrypted_sqlite::{encrypt, sqlite},
    Account,
};
use serde_json::{json, Value};
use std::fs;

pub fn extend(account: &Account) {
    let marker = account.marker;
    let root = account.root();
    let article = format!("Msg_{:x}", md5::compute("gh_news"));
    let sources = [
        ("contact/contact.db", format!(
            "CREATE TABLE contact(id INTEGER,username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,local_type INTEGER,extra_buffer BLOB);
             INSERT INTO contact VALUES(1,'peer','Peer{marker}','',0,1,x'f2010131'),(2,'g@chatroom','Group','',0,1,NULL),(3,'gh_news','News','',8,3,NULL),(4,'other','Same','',0,1,NULL),(5,'another','Same','',0,1,NULL);
             CREATE TABLE contact_label(label_id_,label_name_,sort_order_);
             INSERT INTO contact_label VALUES(1,'G1-{marker}',1),(2,'Ambiguous One',2),(3,'Ambiguous Two',3);
             CREATE TABLE chat_room(id INTEGER,username TEXT,owner TEXT);
             CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER);
             INSERT INTO chat_room VALUES(7,'g@chatroom','peer');
             INSERT INTO chatroom_member VALUES(7,1);")),
        ("session/session.db", format!(
            "CREATE TABLE SessionTable(username TEXT,unread_count INTEGER,summary TEXT,last_timestamp INTEGER,last_msg_type INTEGER,last_msg_sender TEXT,last_sender_display_name TEXT);
             INSERT INTO SessionTable VALUES('peer',2,'history-{marker}',10008,1,'peer',''),('g@chatroom',1,'synthetic group',15,1,'peer',''),('gh_news',1,'synthetic article',20,49,'gh_news','');")),
        ("favorite/favorite.db", format!(
            "CREATE TABLE fav_db_item(local_id INTEGER,type INTEGER,update_time INTEGER,content TEXT,fromusr TEXT,realchatname TEXT);
             INSERT INTO fav_db_item VALUES(7,1,20000,'needle {marker}','author{marker}','peer'),(8,1,10000,'needle older','older','peer'),(9,2,5000,'image','other','peer');")),
        ("message/biz_message_0.db", format!(
            "CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('gh_news');
             CREATE TABLE [{article}](local_id INTEGER,local_type INTEGER,create_time INTEGER,WCDB_CT_message_content INTEGER,message_content TEXT);
             INSERT INTO [{article}] VALUES(1,49,10,0,'<msg><item><title>old</title><url>https://example.test/old</url><pub_time>100</pub_time></item></msg>'),(2,49,20,0,'<msg><item><title>article{marker}</title><url>https://example.test/new</url><pub_time>200</pub_time></item></msg>'),(3,49,15,0,'broken XML');")),
        ("sns/sns.db", format!(
            "CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT);
             INSERT INTO SnsTimeLine VALUES(1,'peer','<TimelineObject><username>peer</username><createTime>10</createTime><contentDesc>needle {marker}</contentDesc></TimelineObject>'),(2,'other','<TimelineObject><username>peer</username><createTime>20</createTime><contentDesc>conflicting author</contentDesc></TimelineObject>');
             CREATE TABLE SnsMessage_tmp3(local_id INTEGER,create_time INTEGER,feed_id INTEGER,from_username TEXT,from_nickname TEXT,content TEXT,is_unread INTEGER);
             INSERT INTO SnsMessage_tmp3 VALUES(1,10,1,'peer','','',1),(2,20,1,'peer','','comment{marker}',0);")),
    ];
    let mut keys: Value =
        serde_json::from_slice(&fs::read(root.join("keys.json")).unwrap()).unwrap();
    for (key, sql) in sources {
        let plain = root.join("g1-build.db");
        let connection = sqlite(&plain);
        connection.execute_batch(&sql).unwrap();
        drop(connection);
        let target = root.join("db_storage").join(key);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        encrypt(&plain, &target);
        keys[key] = json!("11".repeat(32));
    }
    account.seed_keys(&keys);
}
