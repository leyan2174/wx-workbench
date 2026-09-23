//! 全部输入由本模块生成，不读取微信账号。夹具显式构造 HMAC 与 WAL 校验链，
//! 不调用生产校验和函数生成期望值，避免实现和测试共享同一个计算错误。
use super::{atomic, full_decrypt, wal::apply_wal, PAGE_SZ, SQLITE_HDR};
use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use std::{fs, io::Write, path::PathBuf};

const KEY: [u8; 32] = [0x42; 32];
const SALT: [u8; 16] = [0x35; 16];
const FRAME: usize = 24 + PAGE_SZ;

#[test]
fn atomic_output_blocks_parent_rename_until_released() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("output");
    let moved = temp.path().join("moved");
    fs::create_dir(&root).unwrap();
    let output = atomic::Output::new(&root.join("result.db"), &[]).unwrap();
    let held_result = fs::rename(&root, &moved);
    drop(output);
    if held_result.is_ok() {
        fs::rename(&moved, &root).unwrap();
    }
    fs::rename(&root, &moved).unwrap();
    temp.close().unwrap();
    assert!(
        held_result.is_err(),
        "Output allowed parent rename while held"
    );
}

#[test]
fn atomic_output_allows_child_creation_and_new_or_replacement_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("output");
    let path = root.join("result.db");
    for bytes in [b"first".as_slice(), b"replacement".as_slice()] {
        let mut output = atomic::Output::new(&path, &[]).unwrap();
        fs::write(root.join("child"), b"synthetic child").unwrap();
        output.file().write_all(bytes).unwrap();
        output.publish().unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(root.join("child")).unwrap(), b"synthetic child");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
    }
    temp.close().unwrap();
}

fn plain(pgno: u32, marker: u8) -> Vec<u8> {
    let mut bytes = vec![marker; PAGE_SZ];
    if pgno == 1 {
        bytes[..16].copy_from_slice(SQLITE_HDR);
        bytes[16..18].copy_from_slice(&(PAGE_SZ as u16).to_be_bytes());
        bytes[20] = 80;
    }
    bytes[4016..].fill(0);
    bytes
}

fn encrypted(pgno: u32, marker: u8, wal: bool) -> Vec<u8> {
    let bytes = plain(pgno, marker);
    let start = if pgno == 1 && !wal { 16 } else { 0 };
    let iv = [0x24; 16];
    let cipher = cbc::Encryptor::<aes::Aes256>::new((&KEY).into(), (&iv).into())
        .encrypt_padded_vec_mut::<NoPadding>(&bytes[start..4016]);
    let mut page = vec![0; PAGE_SZ];
    if start == 16 {
        page[..16].copy_from_slice(&SALT);
    }
    page[start..4016].copy_from_slice(&cipher);
    page[4016..4032].copy_from_slice(&iv);
    let mut mac_key = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&KEY, &SALT.map(|b| b ^ 0x3a), 2, &mut mac_key);
    let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
    mac.update(&page[start..4032]);
    mac.update(&pgno.to_le_bytes());
    page[4032..].copy_from_slice(&mac.finalize().into_bytes());
    page
}

fn sums(data: &[u8], big: bool, initial: [u32; 2]) -> [u32; 2] {
    let words: Vec<u32> = data
        .chunks_exact(4)
        .map(|word| {
            let bytes = word.try_into().unwrap();
            if big {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        })
        .collect();
    let [mut a, mut b] = initial;
    for pair in words.chunks_exact(2) {
        a = (a as u64 + pair[0] as u64 + b as u64) as u32;
        b = (b as u64 + pair[1] as u64 + a as u64) as u32;
    }
    [a, b]
}

fn wal_bytes(frames: &[(u32, u32, Vec<u8>)], big: bool) -> Vec<u8> {
    let mut out = vec![0; 32];
    out[..4].copy_from_slice(&(if big { 0x377f0683u32 } else { 0x377f0682 }).to_be_bytes());
    out[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
    out[8..12].copy_from_slice(&(PAGE_SZ as u32).to_be_bytes());
    out[16..24].copy_from_slice(&[7; 8]);
    let mut sum = sums(&out[..24], big, [0; 2]);
    out[24..28].copy_from_slice(&sum[0].to_be_bytes());
    out[28..32].copy_from_slice(&sum[1].to_be_bytes());
    for (pgno, commit, page) in frames {
        let mut frame = vec![0; 24];
        frame[..4].copy_from_slice(&pgno.to_be_bytes());
        frame[4..8].copy_from_slice(&commit.to_be_bytes());
        frame[8..16].copy_from_slice(&[7; 8]);
        let payload = [&frame[..8], page.as_slice()].concat();
        sum = sums(&payload, big, sum);
        frame[16..20].copy_from_slice(&sum[0].to_be_bytes());
        frame[20..24].copy_from_slice(&sum[1].to_be_bytes());
        out.extend(frame);
        out.extend(page);
    }
    out
}

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
    output: PathBuf,
    wal: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let output = dir.path().join("output.db");
        let wal = dir.path().join("source.db-wal");
        fs::write(
            &source,
            [
                encrypted(1, 1, false),
                encrypted(2, 2, false),
                encrypted(3, 3, false),
            ]
            .concat(),
        )
        .unwrap();
        fs::write(&output, [plain(1, 1), plain(2, 2), plain(3, 3)].concat()).unwrap();
        Self {
            _dir: dir,
            source,
            output,
            wal,
        }
    }

    fn apply(&self, bytes: &[u8]) -> anyhow::Result<()> {
        fs::write(&self.wal, bytes)?;
        apply_wal(&self.wal, &self.output, &KEY, &self.source)
    }

    fn unchanged_error(&self, bytes: &[u8]) {
        let old = fs::read(&self.output).unwrap();
        assert!(self.apply(bytes).is_err());
        assert_eq!(fs::read(&self.output).unwrap(), old);
    }
}

#[test]
fn full_decrypt_authenticates_all_pages_and_preserves_source() {
    let f = Fixture::new();
    let source = fs::read(&f.source).unwrap();
    full_decrypt(&f.source, &f.output, &KEY).unwrap();
    assert_eq!(
        fs::read(&f.output).unwrap(),
        [plain(1, 1), plain(2, 2), plain(3, 3)].concat()
    );
    assert_eq!(fs::read(&f.source).unwrap(), source);
}

#[test]
fn full_decrypt_wrong_key_and_later_page_tampering_preserve_output() {
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    assert!(full_decrypt(&f.source, &f.output, &[8; 32]).is_err());
    let valid = fs::read(&f.source).unwrap();
    for offset in [PAGE_SZ + 64, PAGE_SZ + 4016, PAGE_SZ + 4032] {
        let mut corrupt = valid.clone();
        corrupt[offset] ^= 1;
        fs::write(&f.source, corrupt).unwrap();
        assert!(full_decrypt(&f.source, &f.output, &KEY).is_err());
        assert_eq!(fs::read(&f.output).unwrap(), old);
    }
    assert_eq!(
        fs::read_dir(f._dir.path()).unwrap().count(),
        2,
        "failed staging must be cleaned"
    );
}

#[test]
fn full_decrypt_rejects_empty_and_partial_pages_without_padding_or_publication() {
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    let valid = fs::read(&f.source).unwrap();
    for length in [0, 16, PAGE_SZ - 1, PAGE_SZ + 17, valid.len() - 1] {
        fs::write(&f.source, &valid[..length]).unwrap();
        assert!(full_decrypt(&f.source, &f.output, &KEY).is_err());
        assert_eq!(fs::read(&f.output).unwrap(), old);
    }
}

#[test]
fn wal_applies_only_last_commit_and_supports_both_checksum_orders() {
    for big in [false, true] {
        let f = Fixture::new();
        let bytes = wal_bytes(
            &[
                (2, 0, encrypted(2, 7, true)),
                (1, 3, encrypted(1, 8, true)),
                (2, 0, encrypted(2, 9, true)),
            ],
            big,
        );
        f.apply(&bytes).unwrap();
        assert_eq!(
            fs::read(&f.output).unwrap(),
            [plain(1, 8), plain(2, 7), plain(3, 3)].concat()
        );
        let once = fs::read(&f.output).unwrap();
        f.apply(&bytes).unwrap();
        assert_eq!(
            fs::read(&f.output).unwrap(),
            once,
            "redo must be idempotent"
        );
    }
}

#[test]
fn wal_multiple_commits_overwrite_pages_and_truncate_to_committed_size() {
    let f = Fixture::new();
    f.apply(&wal_bytes(
        &[
            (2, 3, encrypted(2, 7, true)),
            (2, 0, encrypted(2, 8, true)),
            (1, 2, encrypted(1, 9, true)),
            (3, 0, encrypted(3, 10, true)),
        ],
        false,
    ))
    .unwrap();
    assert_eq!(
        fs::read(&f.output).unwrap(),
        [plain(1, 9), plain(2, 8)].concat()
    );
}

#[test]
fn wal_shrink_then_growth_does_not_resurrect_discarded_pages() {
    let f = Fixture::new();
    f.apply(&wal_bytes(
        &[(1, 1, encrypted(1, 8, true)), (3, 3, encrypted(3, 9, true))],
        false,
    ))
    .unwrap();
    assert_eq!(
        fs::read(&f.output).unwrap(),
        [plain(1, 8), vec![0; PAGE_SZ], plain(3, 9)].concat()
    );
}

#[test]
fn wal_missing_empty_header_only_and_uncommitted_are_noops() {
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    apply_wal(&f.wal, &f.output, &KEY, &f.source).unwrap();
    for bytes in [
        vec![],
        wal_bytes(&[], false),
        wal_bytes(&[(2, 0, encrypted(2, 8, true))], false),
    ] {
        f.apply(&bytes).unwrap();
        assert_eq!(fs::read(&f.output).unwrap(), old);
    }
}

#[test]
fn wal_rejects_malformed_headers_without_modification() {
    let f = Fixture::new();
    for len in [1, 16, 31] {
        f.unchanged_error(&vec![0; len]);
    }
    let valid = wal_bytes(&[(2, 3, encrypted(2, 8, true))], false);
    for offset in [0, 4, 8, 24, 28] {
        let mut bytes = valid.clone();
        bytes[offset] ^= 1;
        f.unchanged_error(&bytes);
    }
}

#[test]
fn wal_rejects_authenticated_oversized_page_and_commit_instead_of_skipping() {
    let f = Fixture::new();
    f.unchanged_error(&wal_bytes(
        &[(1_000_001, 3, encrypted(1_000_001, 8, true))],
        false,
    ));
    f.unchanged_error(&wal_bytes(&[(2, 1_000_001, encrypted(2, 8, true))], false));
}

#[test]
fn wal_never_skips_invalid_middle_frame_to_a_later_commit() {
    for offset in [0, 8, 16, 24 + 100] {
        let f = Fixture::new();
        let mut bytes = wal_bytes(
            &[
                (2, 3, encrypted(2, 7, true)),
                (2, 0, encrypted(2, 8, true)),
                (2, 3, encrypted(2, 9, true)),
            ],
            false,
        );
        let middle = 32 + FRAME;
        if offset == 0 {
            bytes[middle..middle + 4].fill(0);
        } else {
            bytes[middle + offset] ^= 1;
        }
        f.apply(&bytes).unwrap();
        assert_eq!(
            fs::read(&f.output).unwrap(),
            [plain(1, 1), plain(2, 7), plain(3, 3)].concat()
        );
    }
}

#[test]
fn wal_torn_tail_keeps_previous_commit_and_does_not_publish_uncommitted_prefix() {
    let bytes = wal_bytes(
        &[
            (2, 3, encrypted(2, 7, true)),
            (2, 0, encrypted(2, 8, true)),
            (1, 3, encrypted(1, 9, true)),
        ],
        false,
    );
    for remove in [1, PAGE_SZ, FRAME - 1] {
        let f = Fixture::new();
        f.apply(&bytes[..bytes.len() - remove]).unwrap();
        assert_eq!(
            fs::read(&f.output).unwrap(),
            [plain(1, 1), plain(2, 7), plain(3, 3)].concat()
        );
    }
}

#[test]
fn wal_valid_checksums_cannot_mask_bad_hmac_or_wrong_page_number() {
    let f = Fixture::new();
    for pgno in [1, 2] {
        let mut bad = encrypted(pgno, 8, true);
        bad[4032] ^= 1;
        f.unchanged_error(&wal_bytes(&[(pgno, 3, bad)], false));
    }
    // 密文与 HMAC 均为页 2 生成，篡改为页 1 后重算 WAL 校验和仍须失败。
    f.unchanged_error(&wal_bytes(&[(1, 3, encrypted(2, 8, true))], false));
    // 被后续覆盖的旧帧也需要认证，不能以最后一帧正确掩盖错误密钥材料。
    let mut bad = encrypted(2, 8, true);
    bad[4032] ^= 1;
    f.unchanged_error(&wal_bytes(
        &[(2, 0, bad), (2, 3, encrypted(2, 9, true))],
        false,
    ));
}

#[test]
fn wal_uses_explicit_authenticated_source_salt_not_wal_salt_or_inferred_path() {
    let f = Fixture::new();
    let bytes = wal_bytes(&[(1, 3, encrypted(1, 8, true))], false);
    let mut source = fs::read(&f.source).unwrap();
    source[0] ^= 1;
    fs::write(&f.source, source).unwrap();
    f.unchanged_error(&bytes);
}

#[test]
fn wal_page_one_retains_full_payload_layout_without_alternate_format_guessing() {
    let f = Fixture::new();
    f.apply(&wal_bytes(&[(1, 3, encrypted(1, 8, true))], false))
        .unwrap();
    assert_eq!(&fs::read(&f.output).unwrap()[..PAGE_SZ], plain(1, 8));
    f.unchanged_error(&wal_bytes(&[(1, 3, encrypted(1, 9, false))], false));
}

#[test]
fn full_decrypt_and_wal_reject_input_output_aliases_and_hardlinked_outputs() {
    let f = Fixture::new();
    let source = fs::read(&f.source).unwrap();
    assert!(full_decrypt(&f.source, &f.source, &KEY).is_err());
    let alias = f._dir.path().join("alias.db");
    fs::hard_link(&f.source, &alias).unwrap();
    assert!(full_decrypt(&f.source, &alias, &KEY).is_err());
    fs::write(&f.wal, wal_bytes(&[(1, 3, encrypted(1, 8, true))], false)).unwrap();
    assert!(apply_wal(&f.wal, &alias, &KEY, &f.source).is_err());
    assert!(apply_wal(&f.wal, &f.wal, &KEY, &f.source).is_err());
    assert_eq!(fs::read(&f.source).unwrap(), source);
    let output_alias = f._dir.path().join("output-alias.db");
    fs::hard_link(&f.output, &output_alias).unwrap();
    let old = fs::read(&f.output).unwrap();
    assert!(full_decrypt(&f.source, &f.output, &KEY).is_err());
    assert!(apply_wal(&f.wal, &f.output, &KEY, &f.source).is_err());
    assert_eq!(fs::read(&f.output).unwrap(), old);
}

#[test]
fn failed_atomic_publication_preserves_old_output_and_cleans_staging() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    // 模拟另一个只读者禁止替换：写完临时文件后发布失败，旧文件不能被截断。
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&f.output)
        .unwrap();
    assert!(full_decrypt(&f.source, &f.output, &KEY).is_err());
    assert_eq!(fs::read(&f.output).unwrap(), old);
    assert_eq!(fs::read_dir(f._dir.path()).unwrap().count(), 2);
    f.unchanged_error(&wal_bytes(&[(2, 3, encrypted(2, 8, true))], false));
    assert_eq!(fs::read_dir(f._dir.path()).unwrap().count(), 3);
    drop(reader);
    full_decrypt(&f.source, &f.output, &KEY).unwrap();
}

#[test]
fn absent_output_is_not_clobbered_if_another_file_appears_before_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("output.db");
    let mut output = atomic::Output::new(&path, &[]).unwrap();
    output.file().write_all(b"new").unwrap();
    fs::write(&path, b"concurrent").unwrap();
    assert!(output.publish().is_err());
    assert_eq!(fs::read(&path).unwrap(), b"concurrent");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn combined_decrypt_wal_publishes_once_and_preserves_old_on_second_stage_failure() {
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    let mut bad = encrypted(2, 8, true);
    bad[4032] ^= 1;
    fs::write(&f.wal, wal_bytes(&[(2, 3, bad)], false)).unwrap();
    let run = || {
        super::with_staged_output(&f.output, &[&f.source, &f.wal], |temporary| {
            full_decrypt(&f.source, temporary, &KEY)?;
            assert_eq!(fs::read(&f.output)?, old, "DB stage must not publish early");
            apply_wal(&f.wal, temporary, &KEY, &f.source)?;
            assert_eq!(fs::read(&f.output)?, old, "WAL stage must still be private");
            Ok(())
        })
    };
    assert!(run().is_err());
    assert_eq!(fs::read(&f.output).unwrap(), old);
    assert_eq!(fs::read_dir(f._dir.path()).unwrap().count(), 3);
    fs::write(&f.wal, wal_bytes(&[(2, 3, encrypted(2, 8, true))], false)).unwrap();
    run().unwrap();
    assert_eq!(
        fs::read(&f.output).unwrap(),
        [plain(1, 1), plain(2, 8), plain(3, 3)].concat()
    );
    assert_eq!(fs::read_dir(f._dir.path()).unwrap().count(), 3);
}

#[test]
fn combined_staging_rejects_source_change_before_publication() {
    let f = Fixture::new();
    let old = fs::read(&f.output).unwrap();
    assert!(
        super::with_staged_output(&f.output, &[&f.source], |temporary| {
            full_decrypt(&f.source, temporary, &KEY)?;
            // 仅合成文件：模拟另一个写入者在两个阶段之间改变源长度。
            fs::OpenOptions::new()
                .append(true)
                .open(&f.source)?
                .write_all(&[1])?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(&f.output).unwrap(), old);
    assert_eq!(fs::read_dir(f._dir.path()).unwrap().count(), 2);
}

#[test]
fn shared_cache_fixture_builder_produces_authenticated_committed_wal() {
    let f = Fixture::new();
    let pages = [encrypted(1, 8, true), encrypted(2, 9, true)].concat();
    let bytes = super::test_support::wal_bytes(&pages);
    // 与缓存测试共享封装，但认证页面仍由独立夹具产生，真实解密器验证结果。
    f.apply(&bytes).unwrap();
    assert_eq!(
        fs::read(&f.output).unwrap(),
        [plain(1, 8), plain(2, 9)].concat()
    );
}
