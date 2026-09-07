use super::*;
use mcp_image_security::{instrumented_image, publish_probe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn wav() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 1, 0]);
    bytes
}

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

#[test]
fn voice_final_callback_cannot_swap_original_protected_or_output_directory() {
    for mode in ["normal", "protected-swap", "output-swap"] {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("voice-output");
        let protected = root.path().join("protected-final");
        let moved = root.path().join("moved-final");
        fs::create_dir(&output).unwrap();
        fs::create_dir(&protected).unwrap();
        fs::write(protected.join("sentinel.db"), b"preserve").unwrap();
        let guard = publish_probe::Guard::new(&output, &protected).unwrap();
        let original_output = same_file::Handle::from_path(&output).unwrap();
        let bytes = wav();
        let mut callback_ran = false;
        let mut blocked = false;
        let mut destination = PathBuf::new();
        let result = publish_probe::voice(&bytes, &guard, |pending| {
            callback_ran = true;
            destination = pending.path.clone();
            assert!(
                !pending.path.exists(),
                "callback must run before final publication"
            );
            if mode == "protected-swap" {
                fs::rename(&protected, &moved).unwrap();
                fs::create_dir(&protected).unwrap();
            } else if mode == "output-swap" {
                match fs::rename(&output, &moved) {
                    Ok(()) => fs::rename(&protected, &output).unwrap(),
                    Err(error) => {
                        println!("VOICE OUTPUT RENAME BLOCKED IN FINAL CALLBACK: {error}");
                        blocked = true;
                    }
                }
            }
            Ok(())
        });
        println!("VOICE FINAL CALLBACK {mode}: {result:?}");
        assert!(callback_ran);
        if mode == "normal" || blocked {
            assert_eq!(
                original_output,
                same_file::Handle::from_path(&output).unwrap()
            );
            assert_eq!(
                fs::read(protected.join("sentinel.db")).unwrap(),
                b"preserve"
            );
            assert_eq!(fs::read(result.unwrap().path).unwrap(), bytes);
        } else {
            assert!(
                result.is_err(),
                "voice must verify the same guard after the callback"
            );
            assert!(!destination.exists());
            assert!(!moved.join(destination.file_name().unwrap()).exists());
        }
    }
}
