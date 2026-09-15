use super::*;

fn body(from: &str, name: &str) -> String {
    format!("<msg><appmsg><type>+5_7</type><title> reply </title><refermsg><type>1</type><fromusr>{from}</fromusr><displayname>{name}</displayname><content> quoted </content><svrid>18446744073709551615</svrid><createtime>1_234</createtime></refermsg></appmsg></msg>")
}

#[test]
fn typed_reply_preserves_sender_precedence_and_raw_diagnostics() {
    let names = HashMap::from([
        ("self".into(), "Self name".into()),
        ("known".into(), "Known name".into()),
    ]);
    for (chat, from, name, expected) in [
        ("peer", "peer", "Other", "Peer"),
        ("peer", "self", "Other", "me"),
        ("peer", "known", "Other", "Known name"),
        ("peer", "unknown", "Fallback", "Fallback"),
        ("peer", "", "Self name", "me"),
        ("room@chatroom", "unknown", "Fallback", "unknown"),
        ("room@chatroom", "", "Fallback", "Fallback"),
        ("room@chatroom", "self", "Other", "me"),
    ] {
        let parsed = parse_refer(&body(from, name), chat, "Peer", "self", &names).unwrap();
        assert_eq!(parsed.reply.sender_label, expected);
        assert_eq!(parsed.reply.text, "reply");
        assert_eq!(parsed.reply.summary, "quoted");
        assert_eq!(parsed.server_id, "18446744073709551615");
        assert_eq!(parsed.created_at, "1_234");
    }
}

#[test]
fn strict_xml_rejects_wrong_shape_and_hides_payload_in_errors() {
    for xml in [
        "<appmsg><type>57</type><refermsg/></appmsg>",
        "<msg><appmsg><type>49</type><refermsg/></appmsg></msg>",
        "<msg><appmsg><type>57</type></appmsg></msg>",
        "<msg xmlns='private-secret'><appmsg><type>57</type><refermsg/></appmsg></msg>",
        "<!DOCTYPE msg [<!ENTITY x 'private-secret'>]><msg>&x;</msg>",
        "<msg>private-secret",
    ] {
        let error = parse_refer(xml, "peer", "Peer", "", &HashMap::new())
            .err()
            .expect("invalid XML must fail");
        assert!(!error.to_string().contains("private-secret"));
    }
}
