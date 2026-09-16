use super::*;
use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut};

const KEY: &str = "000102030405060708090a0b0c0d0e0f";

fn encrypted(data: &[u8]) -> Vec<u8> {
    let key: Vec<u8> = (0..16).collect();
    let mut buffer = vec![0; data.len() + 16];
    buffer[..data.len()].copy_from_slice(data);
    cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &key)
        .unwrap()
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, data.len())
        .unwrap()
        .to_vec()
}

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn aes_padding_and_errors() {
    for data in [b"GIF89ahello".as_slice(), &[42; 16], &[0; 32]] {
        assert_eq!(decrypt(&encrypted(data), KEY).unwrap(), data);
    }
    // Independently generated with .NET System.Security.Cryptography.Aes:
    // CBC, Padding=None, Key=IV=00..0f, plaintext=[99; 15] followed by tail.
    for (tail, ciphertext) in [
        (0, "c057be34399c3f41e64dba99c0d529a3"),
        (17, "6dfe468b11686b1bc73ab3e424938cf3"),
        (2, "f783f608b3679b5b50cdab10fbdc8392"),
    ] {
        let mut raw = [99; 16];
        raw[15] = tail;
        assert_eq!(decrypt(&unhex(ciphertext), KEY).unwrap(), raw);
    }
    assert_eq!(
        decrypt(b"bad", KEY).unwrap_err().to_string(),
        "invalid encrypted emoji blocks"
    );
    assert_eq!(
        decrypt(&[0; 16], "sensitive-invalid-key")
            .unwrap_err()
            .to_string(),
        "invalid emoji AES key"
    );
}

#[test]
fn independent_padding_vectors() {
    // .NET System.Security.Cryptography.Aes, CBC, Padding=None, Key=IV=00..0f.
    // For pad=1..16: plaintext is [0x41; 32-pad] followed by [pad; pad].
    // Fixed ciphertexts intentionally do not use the Rust encryptor under test.
    let vectors = [
        "265c6c2548555838c5fab3b6887803c8e80f127c96b24d159e53ac9571ec19b7",
        "265c6c2548555838c5fab3b6887803c8cf878d3612f72bc6205a4c53259e126a",
        "265c6c2548555838c5fab3b6887803c8249707b7ebfd598626fa0cdb4564333e",
        "265c6c2548555838c5fab3b6887803c8ce36bee93e29bd8f1c7e8cd22ba6bb2d",
        "265c6c2548555838c5fab3b6887803c8c4b8dfff1a02433bc1380c4fa57b515f",
        "265c6c2548555838c5fab3b6887803c8632e0b12b8eaf5a19218983d135e34de",
        "265c6c2548555838c5fab3b6887803c88d0ec61ad6141a22f04d1ca4275f2f90",
        "265c6c2548555838c5fab3b6887803c886dc97cef3abe96480f49d1dac028e28",
        "265c6c2548555838c5fab3b6887803c8d7be4bd116e9e020bb1322655572cd13",
        "265c6c2548555838c5fab3b6887803c811cd7532606231e373f8b195a84e85f0",
        "265c6c2548555838c5fab3b6887803c8427ee1bd649739a135d5dc3d69f70c51",
        "265c6c2548555838c5fab3b6887803c8d9828294fff8096bb00e91d39fd2ecac",
        "265c6c2548555838c5fab3b6887803c8715ce6f69bf1871ee6813cf11f617f6a",
        "265c6c2548555838c5fab3b6887803c8b0f4203b249dc91cecaa5292c70acc81",
        "265c6c2548555838c5fab3b6887803c8e674e9d53c12a5b81053ee2445a80b8a",
        "265c6c2548555838c5fab3b6887803c88f067f95e559942d2c7d69d93da31dcd",
    ];
    for (index, ciphertext) in vectors.iter().enumerate() {
        assert_eq!(
            decrypt(&unhex(ciphertext), KEY).unwrap(),
            vec![0x41; 31 - index]
        );
    }
    // Same independent generator, plaintext=[16; 16] (all padding).
    assert!(decrypt(&unhex("07feef74e1d5036e900eee118e949293"), KEY)
        .unwrap()
        .is_empty());
}

#[test]
fn fromhex_whitespace_requires_byte_boundaries() {
    let data = encrypted(b"GIF89atest");
    for whitespace in [" ", "\t", "\n", "\r", "\x0b", "\x0c", " \t\r\n\x0b\x0c"] {
        let pairs = KEY
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| std::str::from_utf8(pair).unwrap())
            .collect::<Vec<_>>();
        let key = format!("{whitespace}{}{whitespace}", pairs.join(whitespace));
        assert_eq!(decrypt(&data, &key).unwrap(), b"GIF89atest");
        let broken = format!("{}{whitespace}{}", &KEY[..1], &KEY[1..]);
        assert!(decrypt(&data, &broken).is_err());
    }
    for key in [
        format!("{KEY}\u{a0}"),
        format!("{KEY}00"),
        format!("{KEY}1011121314151617"), // 24-byte AES key is not this format.
        format!("{KEY}101112131415161718191a1b1c1d1e1f"), // Nor is 32-byte.
        KEY[..30].into(),
        KEY[..31].into(),
        "".into(),
    ] {
        assert_eq!(
            decrypt(&data, &key).unwrap_err().to_string(),
            "invalid emoji AES key"
        );
        assert_eq!(
            decrypt(b"", &key).unwrap_err().to_string(),
            "invalid emoji AES key"
        );
    }
    assert!(decrypt(b"", KEY).unwrap().is_empty());
}

#[test]
fn signatures_and_annex_order() {
    for (data, ext) in [
        (b"\xff\xd8\xffx".as_slice(), "jpg"),
        (b"\x89PNG", "png"),
        (b"GIFx", "gif"),
        (b"RIFF", "webp"),
        (b"WXGF", "hevc"),
        (b"nope", "bin"),
        (b"", "bin"),
        (SPS, "bin"),
    ] {
        assert_eq!(detect(data), ext);
    }
    let mut data = vec![0; 250];
    data.extend_from_slice(VPS);
    assert_eq!(detect(&data), "hevc");
    data.insert(0, 0);
    assert_eq!(detect(&data), "bin");
    assert_eq!(hevc_stream(&data).unwrap(), VPS);
    let data = [SPS, b"padding", VPS, b"payload"].concat();
    assert_eq!(hevc_stream(&data).unwrap(), &data[13..]);
    assert_eq!(hevc_stream(SPS).unwrap(), SPS);
    assert_eq!(
        hevc_stream(&[b"WXGF".as_slice(), SPS, b"payload"].concat()).unwrap(),
        [SPS, b"payload"].concat()
    );
    for data in [b"".as_slice(), b"WXGF", &VPS[..5]] {
        assert_eq!(
            hevc_stream(data).unwrap_err().to_string(),
            "emoji HEVC stream missing"
        );
    }
}
