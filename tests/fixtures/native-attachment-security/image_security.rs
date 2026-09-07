use native_image_fixture::{decoder, native_image::{export_image, ImageRequest, MessageIdentity}};
use rusqlite::Connection;
use std::{fs, path::PathBuf};

// 借用已有 fixture 的真实模块接线；仅构造独立审查所需的两表/一 DAT 数据。
const HASH: &str = "0123456789abcdef0123456789abcdef";
struct Account {
    root: tempfile::TempDir,
    db: PathBuf,
    attach: PathBuf,
    output: PathBuf,
    dat: PathBuf,
    message: MessageIdentity,
    plain: Vec<u8>,
}
impl Account {
    fn new(marker: u8) -> Self {
        let root = tempfile::tempdir().unwrap();
        let attach = root.path().join("attach");
        let output = root.path().join("output");
        let resource = root.path().join("resource");
        fs::create_dir(&output).unwrap();
        fs::create_dir(&resource).unwrap();
        let db = resource.join("resource.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id(rowid,user_name) VALUES(7,'synthetic_peer'); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB)").unwrap();
        conn.execute("INSERT INTO MessageResourceInfo VALUES(7,42,3,100,?1)",[HASH.as_bytes()]).unwrap();
        drop(conn);
        let dat = attach.join(format!("{:x}/2026-09/Img/{HASH}.dat",md5::compute(b"synthetic_peer")));
        fs::create_dir_all(dat.parent().unwrap()).unwrap();
        let plain = vec![0xff,0xd8,0xff,marker,marker,0xff,0xd9];
        fs::write(&dat,plain.iter().map(|b|b^0xa5).collect::<Vec<_>>()).unwrap();
        Self { root, db, attach, output, dat, plain, message: MessageIdentity {
            username:"synthetic_peer".into(),source:"message/message_0.db".into(),local_id:42,create_time:100,local_type:3,
        }}
    }
    fn request(&self) -> ImageRequest<'_> {
        ImageRequest {message:&self.message,resource_db:&self.db,attach_root:&self.attach,output_root:&self.output,key:decoder::V2KeyMaterial::default()}
    }
    fn source_bytes(&self) -> (Vec<u8>,Vec<u8>) { (fs::read(&self.db).unwrap(),fs::read(&self.dat).unwrap()) }
}

#[test]
fn image_controlled_accounts_same_full_identity_have_separate_bytes_and_sources() {
    let a=Account::new(b'A');
    let b=Account::new(b'B');
    let before_a=a.source_bytes();
    let before_b=b.source_bytes();
    let oa=export_image(a.request()).unwrap();
    let ob=export_image(b.request()).unwrap();
    assert_eq!(fs::read(&oa.path).unwrap(),a.plain);
    assert_eq!(fs::read(&ob.path).unwrap(),b.plain);
    assert_ne!(oa.decoded_md5,ob.decoded_md5);
    assert_eq!(oa.resource_rowid,ob.resource_rowid);
    assert_eq!(oa.message.local_id,ob.message.local_id);
    assert!(oa.path.starts_with(&a.output));
    assert!(ob.path.starts_with(&b.output));
    assert_eq!(before_a,a.source_bytes());
    assert_eq!(before_b,b.source_bytes());
}

#[test]
fn image_caller_must_bind_resource_and_attach_roots_not_treat_identity_as_auth() {
    let a=Account::new(b'A');
    let b=Account::new(b'B');
    let before_a=a.source_bytes();
    let before_b=b.source_bytes();
    let mut mixed=a.request();
    mixed.resource_db=&b.db;
    // 两账号行内容/资源名相同，模块无法从消息四元组判断根的账号归属。
    let observed=export_image(mixed).unwrap();
    assert_eq!(fs::read(&observed.path).unwrap(),a.plain);
    assert_eq!(observed.binding,"resource_scan_filename_heuristic");
    println!("INTEGRATION PRECONDITION: caller must authenticate paired roots; matching IDs/hashes do not authenticate an account");
    assert_eq!(before_a,a.source_bytes());
    assert_eq!(before_b,b.source_bytes());
}

#[test]
fn image_duplicate_exact_identity_different_resources_is_not_first_match() {
    let a=Account::new(b'A');
    Connection::open(&a.db).unwrap().execute("INSERT INTO MessageResourceInfo VALUES(7,42,3,100,?1)",[b"fedcba9876543210fedcba9876543210".as_slice()]).unwrap();
    let before=a.source_bytes();
    let failure=export_image(a.request()).unwrap_err();
    assert!(failure.to_string().contains("ambiguous exact image resource"),"{failure:#}");
    assert_eq!(before,a.source_bytes());
    assert_eq!(fs::read_dir(&a.output).unwrap().count(),0);
}

#[test]
fn image_missing_timestamp_never_uses_a_later_row() {
    let mut a=Account::new(b'A');
    a.message.create_time=99;
    let failure=export_image(a.request()).unwrap_err();
    assert!(failure.to_string().contains("exact image resource not found"),"{failure:#}");
    assert_eq!(fs::read_dir(&a.output).unwrap().count(),0);
}

#[test]
fn image_output_hardlink_to_resource_db_is_not_overwritten_or_left_with_temp() {
    let a=Account::new(b'A');
    let expected=a.output.join(format!("{:x}.jpg",md5::compute(&a.plain)));
    fs::hard_link(&a.db,&expected).unwrap();
    let before=a.source_bytes();
    assert!(export_image(a.request()).is_err());
    assert_eq!(fs::read(expected).unwrap(),before.0);
    assert_eq!(before,a.source_bytes());
    assert_eq!(fs::read_dir(&a.output).unwrap().count(),1);
}

#[test]
fn image_case_alias_of_attach_root_cannot_become_output_root() {
    let a=Account::new(b'A');
    let alias=a.root.path().join("ATTACH");
    assert!(alias.is_dir());
    let mut request=a.request();
    request.output_root=&alias;
    let before=a.source_bytes();
    let failure=export_image(request).unwrap_err();
    assert!(failure.to_string().contains("outside attachment source"),"{failure:#}");
    assert_eq!(before,a.source_bytes());
}

#[test]
fn image_junction_output_cannot_publish_outside_explicit_root() {
    use std::os::windows::process::CommandExt;
    let a=Account::new(b'A');
    let link=a.root.path().join("output-junction");
    let mut cmd=std::process::Command::new(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap().join("System32").join("cmd.exe"));
    cmd.args(["/d","/c","mklink","/J"]).arg(&link).arg(&a.output).creation_flags(0x08000000);
    println!("COMMAND: {cmd:?}");
    let result=cmd.output().unwrap();
    println!("STDOUT: {}\nSTDERR: {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
    assert!(result.status.success());
    let mut request=a.request(); request.output_root=&link;
    let result=export_image(request);
    // 只删除已核验在临时根内的 junction 本身，不递归其目标。
    assert_eq!(link.parent(),Some(a.root.path()));
    fs::remove_dir(&link).unwrap();
    assert!(result.unwrap_err().to_string().contains("reparse point"));
    assert_eq!(fs::read_dir(&a.output).unwrap().count(),0);
}

#[test]
fn image_oversized_packed_info_fails_before_any_output() {
    let a=Account::new(b'A');
    Connection::open(&a.db).unwrap().execute_batch("UPDATE MessageResourceInfo SET packed_info=zeroblob(1048577)").unwrap();
    let before=a.source_bytes();
    assert!(export_image(a.request()).unwrap_err().to_string().contains("packed_info size limit"));
    assert_eq!(before,a.source_bytes());
    assert_eq!(fs::read_dir(&a.output).unwrap().count(),0);
}
