use super::*;
use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};

const PLAIN: &[u8] = b"\xff\xd8\xffsynthetic sample\xff\xd9";

fn staged(root: &Path) -> StagedSample {
    let material = ImageKeyMaterial {
        aes_key: *b"1234567890abcdef",
        xor_key: 0xa2,
    };
    let mut block = [13u8; 16];
    block[..3].copy_from_slice(&PLAIN[..3]);
    let mut block = GenericArray::clone_from_slice(&block);
    aes::Aes128::new((&material.aes_key).into()).encrypt_block(&mut block);
    let mut data = V2_MAGIC.to_vec();
    data.extend_from_slice(&3u32.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    data.push(0);
    data.extend_from_slice(&block);
    data.extend_from_slice(&PLAIN[3..PLAIN.len() - 2]);
    data.extend(
        PLAIN[PLAIN.len() - 2..]
            .iter()
            .map(|byte| byte ^ material.xor_key),
    );
    PreparedSample {
        guard: HostOutputGuard::new(root).unwrap(),
        output: root.join("sample.jpg"),
        data: Zeroizing::new(data),
    }
    .stage(&material)
    .unwrap()
}

#[test]
fn stage_has_no_final_output_and_shared_publish_writes_real_decoded_bytes() {
    let root = tempfile::tempdir().unwrap();
    let sample = staged(root.path());
    let temporary = sample.temporary.to_path_buf();
    assert!(!sample.output.exists());
    assert_eq!(fs::read(&temporary).unwrap(), PLAIN);
    assert!(OpenOptions::new().write(true).open(&temporary).is_err());
    let report = sample.publish().unwrap();
    assert_eq!(report["bytes"], PLAIN.len());
    assert_eq!(fs::read(root.path().join("sample.jpg")).unwrap(), PLAIN);
    assert!(!temporary.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn dropping_stage_before_publish_removes_internal_material() {
    let root = tempfile::tempdir().unwrap();
    let sample = staged(root.path());
    drop(sample);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn target_race_preserves_existing_output_and_cleans_stage() {
    let root = tempfile::tempdir().unwrap();
    let sample = staged(root.path());
    fs::write(&sample.output, b"existing valid output").unwrap();
    assert!(sample.publish().is_err());
    assert_eq!(
        fs::read(root.path().join("sample.jpg")).unwrap(),
        b"existing valid output"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn invalid_digest_or_size_never_publishes() {
    for oversized in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut sample = staged(root.path());
        if oversized {
            sample.bytes = MAX_DAT_BYTES + 1;
        } else {
            sample.digest[0] ^= 1;
        }
        assert!(sample.publish().is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn replaced_staging_path_is_not_read_as_the_original_source() {
    let root = tempfile::tempdir().unwrap();
    let sample = staged(root.path());
    let moved = root.path().join("moved.bin");
    fs::rename(&sample.temporary, &moved).unwrap();
    fs::write(&sample.temporary, PLAIN).unwrap();
    assert!(sample.publish().is_err());
    assert!(!root.path().join("sample.jpg").exists());
    assert_eq!(fs::read(&moved).unwrap(), PLAIN);
}

#[test]
fn additional_source_hardlink_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let sample = staged(root.path());
    let alias = root.path().join("alias.bin");
    fs::hard_link(&sample.temporary, &alias).unwrap();
    assert!(sample.publish().is_err());
    assert!(!root.path().join("sample.jpg").exists());
    assert_eq!(fs::read(&alias).unwrap(), PLAIN);
}
