use super::*;
use mcp_image_security::{instrumented_image, publish_probe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[test]
fn image_original_guard_is_rechecked_after_staged_sync() {
    for mode in ["normal", "protected-swap", "output-swap"] {
        let f = Account::new(b'A');
        let protected = f.root.path().join("protected-final");
        let moved = f.root.path().join("moved-final");
        fs::create_dir(&protected).unwrap();
        fs::write(protected.join("sentinel.db"), b"preserve").unwrap();
        let guard = publish_probe::Guard::new(&f.output, &protected).unwrap();
        let original_output = same_file::Handle::from_path(&f.output).unwrap();
        let identity = instrumented_image::MessageIdentity {
            username: CHAT.into(),
            source: "message/message_0.db".into(),
            local_id: 42,
            create_time: 100,
            local_type: 3,
        };
        let attach = f.db.db_dir().parent().unwrap().join("msg/attach");
        let before = f.sources();
        let output = f.output.clone();
        let protected_copy = protected.clone();
        let moved_copy = moved.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let witness = fired.clone();
        let blocked = Arc::new(AtomicBool::new(false));
        let blocked_hook = blocked.clone();
        let result = publish_probe::image(
            instrumented_image::ImageRequest {
                message: &identity,
                resource_db: &f.resource,
                attach_root: &attach,
                output_root: &f.output,
                key: V2KeyMaterial::default(),
            },
            &guard,
            move || {
                witness.store(true, Ordering::SeqCst);
                if mode == "protected-swap" {
                    fs::rename(&protected_copy, &moved_copy).unwrap();
                    fs::create_dir(&protected_copy).unwrap();
                } else if mode == "output-swap" {
                    match fs::rename(&output, &moved_copy) {
                        Ok(()) => fs::rename(&protected_copy, &output).unwrap(),
                        Err(error) => {
                            println!("IMAGE OUTPUT RENAME BLOCKED AFTER SYNC: {error}");
                            blocked_hook.store(true, Ordering::SeqCst);
                        }
                    }
                }
            },
        );
        println!("IMAGE AFTER SYNC {mode}: {result:?}");
        assert!(fired.load(Ordering::SeqCst));
        assert_eq!(f.sources(), before);
        if mode == "normal" || blocked.load(Ordering::SeqCst) {
            assert_eq!(
                original_output,
                same_file::Handle::from_path(&f.output).unwrap()
            );
            assert_eq!(
                fs::read(protected.join("sentinel.db")).unwrap(),
                b"preserve"
            );
            assert_eq!(fs::read(result.unwrap().path).unwrap(), f.plain);
        } else {
            assert!(
                result.is_err(),
                "the originally approved identities must survive until final commit"
            );
            assert!(!f.destination().exists());
            assert!(!moved.join(f.destination().file_name().unwrap()).exists());
        }
    }
}
