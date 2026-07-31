# Preview Native Backends + Content Sniffing — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes.

**Goal:** Replace the preview shell-outs to `unzip`, `tar`, `openssl` and
(for image/audio/generic) `mediainfo` with pure-Rust backends, keeping each
shell-out as a last-resort fallback; fix the gz-of-non-tar mis-dispatch; and
add content sniffing to `get_mime_type` so extensionless/mislabeled files
route correctly. `ffmpeg` and `bat` stay by design.

**Architecture:** Every converted dispatch arm in `FilePreview::new`
(`src/panel/preview.rs:225`) gets a `*_preview` wrapper that tries a
`native_*` function (pure Rust, `anyhow::Result<Vec<String>>`) and on `Err`
logs at `debug` and falls back to today's external command, which itself
falls back to error text — a preview is never lost. All listers keep the
128-line cap. `get_mime_type` (`src/engine/opener.rs:22`) gains a bounded
256-byte sniff that runs **only** when the extension is unhelpful (none, or
`mime_guess` has no match / says octet-stream).

**Tech Stack:** new crates `zip` (>=7.2, <8; 8.0 needs rustc 1.88),
`tar` 0.4, `flate2` 1, `x509-parser` 0.18, `lofty` (>=0.22, <0.22.3;
0.22.3 needs 1.85), `infer` 0.22 — the whole set was **compile- and
test-verified on rustc 1.83.0** (the MSRV) with the lock pins of Task 1.
Existing deps reused: `image`, `unix_mode`, `time`, `once_cell`,
`tempfile` (tests).

**Design doc:** `docs/plans/2026-07-30-preview-native-backends-design.md`

**Resolved design ambiguities** (deviations are deliberate):
- A3 says the `tar_list` zombie dance is removed "entirely"; the fallback
  principle says keep every shell-out. Resolution: `tar_list` stays, but
  **only as the fallback** — the primary path never spawns tar. Its
  existing `external_cmd_tests` stay valid.
- A6's stat block cannot fail short of an unreadable file, so `mediainfo`
  remains wired as the fallback arm but effectively stops running for
  generic `application/*` — matching "stays only as the fallback".
- A5: lofty's `primary_tag()` is `None` for RIFF-INFO-tagged WAVs (WAV's
  primary tag type is ID3v2), so tags are read via
  `primary_tag().or_else(|| first_tag())` (verified against lofty 0.22.2).
- B: today an unknown *extension* already yields `text/plain`
  (`first_or_text_plain`). Sniffing keeps that as the final fallback, so
  no behaviour regresses for weird-but-texty extensions. There are no
  existing tests asserting the old extensionless behaviour (verified:
  `get_mime_type` has no tests; callers are preview.rs, util.rs
  `print_metadata`, styles.rs, opener.rs `open`).

**Per-task rule:** every task ends with `cargo build`, `cargo test`,
`cargo clippy --all-targets` all green, then a commit (trailer:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`). Do not proceed
on a red build. New preview tests live in a new
`#[cfg(test)] mod native_backend_tests` in preview.rs (same
tempfile-fixture style and sentence-style names as `external_cmd_tests`).

---

### Task 1: Dependencies (MSRV-1.83-verified pins)

**Files:**
- Modify: `Cargo.toml` (dependency table, keep alphabetical order)
- Modify: `Cargo.lock` (via the pin commands below; the lock is committed,
  so the pins are durable)

**Step 1:** Add to `[dependencies]` (comment style mirrors the `trash` pin):

```toml
flate2 = "1"
infer = "0.22"
# lofty 0.22.3 bumps MSRV to 1.85; cap below it (its ogg_pager dep 0.7.1
# does too - pinned to 0.7.0 in Cargo.lock).
lofty = ">=0.22, <0.22.3"
tar = "0.4.44"
x509-parser = "0.18"
# zip 8.0 bumps MSRV to 1.88; cap below it.
zip = { version = ">=7.2, <8", default-features = false, features = ["deflate"] }
```

**Step 2:** Pin the two fresh transitive deps whose newest versions need
rustc ≥1.85 (everything else — `time-core` 0.1.2, `indexmap` 2.7.0,
`time` 0.3.37 — is already held back by the committed Cargo.lock):

```bash
cargo update -p ogg_pager --precise 0.7.0   # 0.7.1 needs 1.85, 0.7.2 needs 1.89
cargo update -p uuid --precise 1.12.1       # via infer→cfb; 1.24 needs 1.85
```

**Step 3:** `cargo build && cargo test` — green (verified: this exact set
resolves to zip 7.2.0, tar 0.4.46, flate2 1.1.9, x509-parser 0.18.1,
lofty 0.22.2, infer 0.22.0 and passes all 124 tests on rustc 1.83.0).
If cargo reports another `edition2024` / "requires rustc" package, pin it
the same way and note it in the commit message.

**Step 4:** Commit: `chore(deps): native preview backend crates (MSRV 1.83 pins)`

---

### Task 2: A1 — native image info lines (drop mediainfo for images)

**Files:**
- Modify: `src/panel/preview.rs` (image arm ~line 226, new helpers near
  `image_preview`, tests in new `mod native_backend_tests`)

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod native_backend_tests {
    use super::*;

    #[test]
    fn image_info_lines_contain_dimensions_format_and_size() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(64, 48));
        let lines = image_info_lines(&img, 1234, SystemTime::UNIX_EPOCH, "png");
        let joined = lines.join("\n");
        assert!(joined.contains("64 × 48"), "{joined}");
        assert!(joined.contains("Rgb8"), "{joined}");
        assert!(joined.contains("png"), "{joined}");
        assert!(joined.contains("1.2 K"), "{joined}");
    }

    #[test]
    fn native_image_preview_populates_info_without_mediainfo() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tiny.png");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, info } => {
                assert!(img.is_some());
                assert!(info.iter().any(|l| l.contains("8 × 8")), "{info:?}");
            }
            _ => panic!("expected an image preview"),
        }
    }
}
```

**Step 2:** `cargo test native_backend` — FAIL (not defined).

**Step 3: Implement.** Info from the decode + metadata; no process spawn.
Dimensions are read **before** the 960×540 thumbnail shrink:

```rust
/// Info footer for an image preview, built from the decoded image and
/// file metadata instead of a mediainfo shell-out.
fn image_info_lines(
    img: &DynamicImage,
    byte_size: u64,
    modified: SystemTime,
    subtype: &str,
) -> Vec<String> {
    use time::OffsetDateTime;
    let t = OffsetDateTime::from(modified);
    vec![
        format!("{} × {}  {:?}", img.width(), img.height(), img.color()),
        format!("{subtype} · {}", crate::util::file_size_str(byte_size)),
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year(), u8::from(t.month()), t.day(), t.hour(), t.minute(), t.second()
        ),
    ]
}

/// Image arm: decode once, derive the info lines from the decode itself.
/// `image_preview` stays untouched - the video path feeds it thumbnails.
fn native_image_preview(path: &Path, mime: &mime::Mime) -> Preview {
    let meta = path.metadata().ok();
    let byte_size = meta.as_ref().map(|m| m.len()).unwrap_or_default();
    let modified = meta
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    match image::io::Reader::open(path).ok().and_then(|r| r.decode().ok()) {
        Some(img) => {
            let info = image_info_lines(&img, byte_size, modified, mime.subtype().as_str());
            Preview::Image { img: Some(img.thumbnail(960, 540)), info }
        }
        None => Preview::Image { img: None, info: Vec::new() },
    }
}
```

In `FilePreview::new` change the arm to
`("image", _) => native_image_preview(&path, &mime),` — this deletes the
`mediainfo(&path)` call for images (one shell-out per image visit gone).
`file_size_str` needs re-export or `crate::util::` path (it is `pub`).

**Step 4:** `cargo test native_backend` — PASS (also with mediainfo
uninstalled — nothing is spawned).

**Step 5:** Commit: `feat(preview): native image info lines, drop mediainfo for images`

---

### Task 3: A2 — native zip listing via the `zip` crate

**Files:**
- Modify: `src/panel/preview.rs` (zip arm ~line 232, new functions, tests)
- Modify: `src/engine/opener.rs` (`extract`, zip branch ~line 339 —
  comment only)

**Step 1: Write the failing tests** (fixtures built with the crate under
test, mirroring how `make_tar` uses the real tar binary today)

```rust
/// Builds `dir/archive.zip` containing `files` via the zip crate.
fn make_zip(dir: &Path, files: &[String]) -> PathBuf {
    use std::io::Write;
    let archive = dir.join("archive.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for name in files {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(b"content").unwrap();
    }
    writer.finish().unwrap();
    archive
}

#[test]
fn native_zip_list_returns_all_names_of_a_small_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let files: Vec<String> = ["a.txt", "b.txt", "dir/c.txt"]
        .iter().map(|s| s.to_string()).collect();
    let archive = make_zip(tmp.path(), &files);
    let lines = native_zip_list(&archive).unwrap();
    assert_eq!(lines.len(), 3);
    for (line, name) in lines.iter().zip(&files) {
        assert!(line.contains(name.as_str()), "{line}");
    }
}

#[test]
fn native_zip_list_caps_the_listing_at_128_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let files: Vec<String> = (0..130).map(|i| format!("file-{i:03}.txt")).collect();
    let archive = make_zip(tmp.path(), &files);
    assert_eq!(native_zip_list(&archive).unwrap().len(), 128);
}

#[test]
fn native_zip_list_on_garbage_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let bogus = tmp.path().join("not-a.zip");
    std::fs::write(&bogus, b"definitely not a zip").unwrap();
    // Err is what triggers the unzip fallback in zip_preview().
    assert!(native_zip_list(&bogus).is_err());
}
```

**Step 2:** `cargo test native_zip_list` — FAIL.

**Step 3: Implement** (central-directory read only, no decompression —
`by_index_raw` never inflates):

```rust
/// List a zip archive natively: `size  name` per entry, capped at 128.
fn native_zip_list(path: &Path) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut lines = Vec::new();
    for i in 0..archive.len().min(128) {
        let entry = archive.by_index_raw(i)?;
        lines.push(format!(
            "{:>8}  {}",
            crate::util::file_size_str(entry.size()),
            entry.name()
        ));
    }
    Ok(lines)
}

/// Native-first zip arm; unzip stays as the shell-out fallback.
fn zip_preview(path: &Path) -> Preview {
    match native_zip_list(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native zip list failed, trying unzip: {e}");
            cmd_to_preview(
                "unzip",
                std::process::Command::new("unzip")
                    .arg("-l")
                    .arg(path)
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            )
        }
    }
}
```

Arm: `("application", "zip") => zip_preview(&path),` (the inline unzip
invocation moves into `zip_preview`). In `opener.rs`'s `extract` zip
branch add the design-mandated note:
`// NOTE: the zip crate (already a dependency for previews) could take over extraction here.`

**Step 4:** `cargo test native_zip_list` — PASS.

**Step 5:** Commit: `feat(preview): native zip listing via the zip crate`

---

### Task 4: A3a — native tar listing via the `tar` crate

**Files:**
- Modify: `src/panel/preview.rs` (x-tar arm ~line 231, new functions, tests)

**Step 1: Write the failing tests** (fixture via `tar::Builder`, so the
test also runs where GNU tar is absent; the *fallback* tests keep using
the binary as before)

```rust
/// Builds `dir/archive.tar` containing `files` via the tar crate.
fn make_native_tar(dir: &Path, files: &[String]) -> PathBuf {
    let archive = dir.join("archive.tar");
    let mut builder = tar::Builder::new(std::fs::File::create(&archive).unwrap());
    for name in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name.as_str(), &b"content"[..]).unwrap();
    }
    builder.finish().unwrap();
    archive
}

#[test]
fn native_tar_list_returns_all_names_of_a_small_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let files: Vec<String> = ["a.txt", "b.txt"].iter().map(|s| s.to_string()).collect();
    let archive = make_native_tar(tmp.path(), &files);
    let lines = native_tar_list(File::open(archive).unwrap()).unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("a.txt") && lines[0].contains("rw-r--r--"), "{lines:?}");
}

#[test]
fn native_tar_list_caps_the_listing_at_128_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let files: Vec<String> = (0..130).map(|i| format!("file-{i:03}.txt")).collect();
    let archive = make_native_tar(tmp.path(), &files);
    assert_eq!(native_tar_list(File::open(archive).unwrap()).unwrap().len(), 128);
}

#[test]
fn native_tar_list_on_garbage_is_an_error() {
    let garbage = std::io::Cursor::new(vec![0xffu8; 1024]);
    assert!(native_tar_list(garbage).is_err());
}
```

**Step 2:** `cargo test native_tar_list` — FAIL.

**Step 3: Implement.** Generic over `Read` so Task 5 can feed it a
gzip-decoding stream; streaming, so only the first 128 headers are read
even from a huge archive (this is what makes the kill-before-reap dance
unnecessary on the primary path):

```rust
/// List a tar stream natively: `mode size name` per entry, capped at 128.
fn native_tar_list<R: io::Read>(reader: R) -> anyhow::Result<Vec<String>> {
    let mut archive = tar::Archive::new(reader);
    let mut lines = Vec::new();
    for entry in archive.entries()?.take(128) {
        let entry = entry?;
        let header = entry.header();
        lines.push(format!(
            "{} {:>8}  {}",
            unix_mode::to_string(header.mode().unwrap_or(0)),
            crate::util::file_size_str(header.size().unwrap_or(0)),
            entry.path()?.display()
        ));
    }
    Ok(lines)
}

/// Native-first x-tar arm; the tar binary stays as the fallback (the
/// zombie-reaping tar_list() is now only reachable through it).
fn tar_preview(path: &Path) -> Preview {
    let native = File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(native_tar_list);
    match native {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native tar list failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}
```

Arm: `("application", "x-tar") => tar_preview(&path),`. Note: `unix_mode`
masks the file-type bits itself; header modes like 0o644 render as
`-rw-r--r--`-style strings.

**Step 4:** `cargo test native_tar_list` — PASS (and the existing
`tar_list_*` fallback tests still pass).

**Step 5:** Commit: `feat(preview): native tar listing via the tar crate`

---

### Task 5: A3b — gzip arm: decompress, sniff tar magic, fix the gz-of-non-tar mis-dispatch

**Files:**
- Modify: `src/panel/preview.rs` (gzip arm ~line 230, new functions, tests)

**Step 1: Write the failing tests**

```rust
/// Builds `dir/archive.tar.gz` via tar::Builder into a GzEncoder.
fn make_native_tar_gz(dir: &Path, files: &[String]) -> PathBuf {
    let archive = dir.join("archive.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&archive).unwrap(),
        flate2::Compression::default(),
    );
    let mut builder = tar::Builder::new(encoder);
    for name in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name.as_str(), &b"content"[..]).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
    archive
}

#[test]
fn native_gz_preview_lists_the_members_of_a_tar_gz() {
    let tmp = tempfile::tempdir().unwrap();
    let files: Vec<String> = ["a.txt", "b.txt"].iter().map(|s| s.to_string()).collect();
    let archive = make_native_tar_gz(tmp.path(), &files);
    match gz_preview(&archive) {
        Preview::Text { lines } => {
            assert!(lines.iter().any(|l| l.contains("a.txt")), "{lines:?}");
            assert!(lines.iter().any(|l| l.contains("b.txt")), "{lines:?}");
        }
        _ => panic!("expected a text preview"),
    }
}

#[test]
fn gz_of_a_non_tar_file_previews_the_decompressed_text() {
    // The old dispatch sent every application/gzip to `tar --list`, so a
    // plain foo.txt.gz produced an error instead of its content.
    let tmp = tempfile::tempdir().unwrap();
    let gz = tmp.path().join("foo.txt.gz");
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(
        std::fs::File::create(&gz).unwrap(),
        flate2::Compression::default(),
    );
    enc.write_all(b"hello from a gzipped text file\nsecond line\n").unwrap();
    enc.finish().unwrap();
    match gz_preview(&gz) {
        Preview::Text { lines } => {
            assert!(lines[0].contains("hello from a gzipped text file"), "{lines:?}");
        }
        _ => panic!("expected a text preview"),
    }
}
```

**Step 2:** `cargo test gz_` — FAIL.

**Step 3: Implement.** Decompress the head, check the tar magic
(`ustar` at offset 257 — covers both POSIX `ustar\0` and GNU `ustar  `),
then either chain head+rest into `native_tar_list` or show the
decompressed head as text (bounded 64 KiB read, lossy UTF-8, 128 lines,
`\r` scrubbed like `bat_preview`):

```rust
/// gzip arm: tar.gz gets a member listing, a gzipped non-tar file gets
/// its decompressed head as text. Falls back to the tar binary only
/// when the gzip stream itself is unreadable.
fn gz_preview(path: &Path) -> Preview {
    match native_gz_preview(path) {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!("native gzip preview failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

fn native_gz_preview(path: &Path) -> anyhow::Result<Preview> {
    let mut decoder = flate2::read::GzDecoder::new(File::open(path)?);
    // Read up to one tar block; a short read just means a small file.
    let mut head = [0u8; 512];
    let mut filled = 0;
    while filled < head.len() {
        let n = decoder.read(&mut head[filled..])?;
        if n == 0 { break; }
        filled += n;
    }
    let rest = io::Cursor::new(head[..filled].to_vec()).chain(decoder);
    if filled >= 262 && &head[257..262] == b"ustar" {
        return Ok(Preview::Text { lines: native_tar_list(rest)? });
    }
    // Not a tar: show the decompressed head as text (bounded).
    let mut buf = Vec::with_capacity(64 * 1024);
    rest.take(64 * 1024).read_to_end(&mut buf)?;
    let lines = String::from_utf8_lossy(&buf)
        .lines()
        .take(128)
        .map(|l| l.replace('\r', ""))
        .collect();
    Ok(Preview::Text { lines })
}
```

Arm: `("application", "gzip") => gz_preview(&path),`. Needs
`use std::io::Read;` (adjust the existing `io` import).

**Step 4:** `cargo test gz_` — PASS.

**Step 5:** Commit: `feat(preview): native gzip handling, fix gz-of-non-tar mis-dispatch`

---

### Task 6: A4 — native certificate preview via `x509-parser`

**Files:**
- Modify: `src/panel/preview.rs` (`cert_preview` ~line 454, tests)

**Step 1: Generate the fixture once and paste it.** Run
`openssl req -x509 -newkey rsa:2048 -keyout /dev/null -out /dev/stdout -days 3650 -nodes -subj "/CN=rfm-test/O=Example Org" -addext "subjectAltName=DNS:example.test"`
and embed the output as `const TEST_PEM: &str = "-----BEGIN CERTIFICATE-----\n…";`
in the test module (deterministic — no openssl needed at test time; 3650
days so the printed validity stays sane for years).

**Step 2: Write the failing tests**

```rust
#[test]
fn native_cert_lines_include_subject_validity_and_san() {
    let lines = native_cert_lines(TEST_PEM.as_bytes()).unwrap();
    let joined = lines.join("\n");
    assert!(joined.contains("rfm-test"), "{joined}");
    assert!(joined.contains("Not before"), "{joined}");
    assert!(joined.contains("Not after"), "{joined}");
    assert!(joined.contains("example.test"), "{joined}");
}

#[test]
fn native_cert_lines_on_garbage_are_an_error() {
    // Err is what routes cert_preview to the openssl/bat fallback.
    assert!(native_cert_lines(b"not a certificate").is_err());
}
```

**Step 3:** `cargo test native_cert` — FAIL.

**Step 4: Implement** (API verified against x509-parser 0.18.1):

```rust
/// Parse a PEM or DER certificate and print the fields people actually
/// inspect. Errors route the caller to the openssl/bat fallback.
fn native_cert_lines(data: &[u8]) -> anyhow::Result<Vec<String>> {
    use x509_parser::prelude::*;
    let der: Vec<u8>;
    let cert_der: &[u8] = if data.starts_with(b"-----BEGIN") {
        let (_, pem) = parse_x509_pem(data)?;
        der = pem.contents;
        &der
    } else {
        data
    };
    let (_, cert) = X509Certificate::from_der(cert_der)?;
    let mut lines = vec![
        format!("Subject:    {}", cert.subject()),
        format!("Issuer:     {}", cert.issuer()),
        format!("Not before: {}", cert.validity().not_before),
        format!("Not after:  {}", cert.validity().not_after),
        format!("Serial:     {}", cert.raw_serial_as_string()),
        format!("Sig. alg.:  {}", cert.signature_algorithm.algorithm),
    ];
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for name in &san.value.general_names {
            lines.push(format!("SAN:        {name}"));
        }
    }
    Ok(lines)
}
```

Rework `cert_preview`: rename the current body (openssl probe + shell-out,
bat fallback) to `cert_preview_external(path)` unchanged, and make
`cert_preview` native-first:

```rust
fn cert_preview(path: impl AsRef<Path>) -> Preview {
    let native = std::fs::read(path.as_ref())
        .map_err(anyhow::Error::from)
        .and_then(|data| native_cert_lines(&data));
    match native {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native cert parse failed, trying openssl: {e}");
            cert_preview_external(path)
        }
    }
}
```

The dispatch arm (`("application", "x-x509-ca-cert")`) is unchanged.

**Step 5:** `cargo test native_cert` — PASS.

**Step 6:** Commit: `feat(preview): native certificate preview via x509-parser`

---

### Task 7: A5 — native audio metadata via `lofty`

**Files:**
- Modify: `src/panel/preview.rs` (audio arm ~line 227, new functions, tests)

**Step 1: Write the failing tests.** The fixture is synthesized: a minimal
44-byte-header PCM WAV written byte-by-byte, then tagged *with lofty
itself* — no committed binary, no external tool:

```rust
/// Minimal PCM WAV (1 s of silence, 8 kHz mono 8-bit), tagged via lofty.
fn make_tagged_wav(dir: &Path) -> PathBuf {
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};
    let path = dir.join("tone.wav");
    let data_len: u32 = 8000;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());      // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());      // mono
    wav.extend_from_slice(&8000u32.to_le_bytes());   // sample rate
    wav.extend_from_slice(&8000u32.to_le_bytes());   // byte rate
    wav.extend_from_slice(&1u16.to_le_bytes());      // block align
    wav.extend_from_slice(&8u16.to_le_bytes());      // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&vec![128u8; data_len as usize]);
    std::fs::write(&path, wav).unwrap();
    let mut tag = Tag::new(TagType::RiffInfo);
    tag.set_title("Test Title".to_string());
    tag.set_artist("Test Artist".to_string());
    tag.save_to_path(&path, lofty::config::WriteOptions::default()).unwrap();
    path
}

#[test]
fn native_audio_lines_include_title_and_duration() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = make_tagged_wav(tmp.path());
    let lines = native_audio_lines(&wav).unwrap();
    let joined = lines.join("\n");
    assert!(joined.contains("Test Title"), "{joined}");
    assert!(joined.contains("Duration"), "{joined}");
    assert!(joined.contains("8000"), "sample rate expected: {joined}");
}

#[test]
fn native_audio_lines_on_garbage_are_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let bogus = tmp.path().join("noise.mp3");
    std::fs::write(&bogus, b"not audio at all").unwrap();
    assert!(native_audio_lines(&bogus).is_err());
}
```

**Step 2:** `cargo test native_audio` — FAIL.

**Step 3: Implement** (API verified against lofty 0.22.2; note the
`primary_tag().or_else(first_tag)` — WAV's *primary* tag type is ID3v2, so
`primary_tag()` alone misses RIFF-INFO tags):

```rust
/// Curated audio block via lofty: format/duration/bitrate + common tags.
fn native_audio_lines(path: &Path) -> anyhow::Result<Vec<String>> {
    use lofty::prelude::*;
    let tagged = lofty::read_from_path(path)?;
    let props = tagged.properties();
    let secs = props.duration().as_secs();
    let mut lines = vec![
        format!("Format:      {:?}", tagged.file_type()),
        format!("Duration:    {}:{:02}", secs / 60, secs % 60),
    ];
    if let Some(bitrate) = props.audio_bitrate() {
        lines.push(format!("Bitrate:     {bitrate} kbps"));
    }
    if let Some(rate) = props.sample_rate() {
        lines.push(format!("Sample rate: {rate} Hz"));
    }
    if let Some(channels) = props.channels() {
        lines.push(format!("Channels:    {channels}"));
    }
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        lines.push(String::new());
        if let Some(title) = tag.title() {
            lines.push(format!("Title:       {title}"));
        }
        if let Some(artist) = tag.artist() {
            lines.push(format!("Artist:      {artist}"));
        }
        if let Some(album) = tag.album() {
            lines.push(format!("Album:       {album}"));
        }
        if let Some(year) = tag.year() {
            lines.push(format!("Year:        {year}"));
        }
    }
    Ok(lines)
}

/// Native-first audio arm; mediainfo stays as the shell-out fallback.
fn audio_preview(path: &Path) -> Preview {
    match native_audio_lines(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native audio metadata failed, trying mediainfo: {e}");
            cmd_to_preview("mediainfo", mediainfo(path))
        }
    }
}
```

Arm: `("audio", _) => audio_preview(&path),`.

**Step 4:** `cargo test native_audio` — PASS.

**Step 5:** Commit: `feat(preview): native audio metadata via lofty`

---

### Task 8: A6 — generic `application/*` stat block

**Files:**
- Modify: `src/panel/preview.rs` (catch-all application arm ~line 254,
  new functions, tests)

**Step 1: Write the failing test**

```rust
#[test]
fn stat_block_lines_contain_size_permissions_and_mime() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("blob.bin");
    std::fs::write(&path, vec![0u8; 2048]).unwrap();
    let mime: mime::Mime = "application/octet-stream".parse().unwrap();
    let lines = stat_block_lines(&path, &mime).unwrap();
    let joined = lines.join("\n");
    assert!(joined.contains("blob.bin"), "{joined}");
    assert!(joined.contains("2.0 K"), "{joined}");
    assert!(joined.contains("application/octet-stream"), "{joined}");
    assert!(joined.contains("rw-"), "permission string expected: {joined}");
}
```

**Step 2:** `cargo test stat_block` — FAIL.

**Step 3: Implement.** Dependency-free (reuses `unix_mode`, `time`,
`file_size_str` — all existing deps), same field style as
`util::print_metadata`:

```rust
/// Dependency-free stat block for generic application/* files: path,
/// size, mtime, MIME type, permissions. Replaces the mediainfo
/// boilerplate; an Err routes to the mediainfo fallback.
fn stat_block_lines(path: &Path, mime: &mime::Mime) -> io::Result<Vec<String>> {
    use std::os::unix::fs::PermissionsExt;
    use time::OffsetDateTime;
    let meta = path.metadata()?;
    let modified = meta
        .modified()
        .map(OffsetDateTime::from)
        .map(|t| {
            format!(
                "{}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(), u8::from(t.month()), t.day(), t.hour(), t.minute(), t.second()
            )
        })
        .unwrap_or_else(|_| String::from("cannot read timestamp"));
    Ok(vec![
        format!("{}", path.display()),
        String::new(),
        format!("Size:        {}", crate::util::file_size_str(meta.len())),
        format!("Modified:    {modified}"),
        format!("MIME type:   {mime}"),
        format!("Permissions: {}", unix_mode::to_string(meta.permissions().mode())),
    ])
}

/// Generic application/* arm: native stat block, mediainfo only as the
/// fallback for the rare unreadable-metadata case.
fn stat_preview(path: &Path, mime: &mime::Mime) -> Preview {
    match stat_block_lines(path, mime) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("stat block failed, trying mediainfo: {e}");
            cmd_to_preview("mediainfo", mediainfo(path))
        }
    }
}
```

Arm: `("application", _) => stat_preview(&path, &mime),`. The text-based
and binary-based `application/*` arms above it are untouched (they still
win — match order).

**Step 4:** `cargo test stat_block` — PASS.

**Step 5:** Commit: `feat(preview): native stat block for generic application types`

---

### Task 9: B — content sniffing in `get_mime_type`

**Files:**
- Modify: `src/engine/opener.rs` (`get_mime_type` ~line 22, new
  `sniff_mime`/`sniffed` helpers, new `#[cfg(test)] mod mime_tests`)

**Step 1: Write the failing tests** (pure byte-slice tests first, then
tempfile integration through the public `get_mime_type`)

```rust
#[cfg(test)]
mod mime_tests {
    use super::*;

    #[test]
    fn sniff_mime_detects_a_shebang_as_text() {
        assert_eq!(sniff_mime(b"#!/bin/sh\necho hi\n"), Some(mime::TEXT_PLAIN));
    }

    #[test]
    fn sniff_mime_detects_pdf_zip_and_gzip_magic() {
        assert_eq!(sniff_mime(b"%PDF-1.4 rest").unwrap().to_string(), "application/pdf");
        assert_eq!(sniff_mime(b"PK\x03\x04rest").unwrap().to_string(), "application/zip");
        assert_eq!(sniff_mime(b"\x1f\x8b\x08rest").unwrap().to_string(), "application/gzip");
    }

    #[test]
    fn sniff_mime_treats_mostly_printable_utf8_as_text() {
        assert_eq!(sniff_mime("kein shebang, nur Text — ümlaute ok\n".as_bytes()),
                   Some(mime::TEXT_PLAIN));
    }

    #[test]
    fn sniff_mime_returns_none_for_random_binary() {
        assert_eq!(sniff_mime(&[0x00, 0x01, 0x02, 0xfe, 0xff, 0x00]), None);
    }

    #[test]
    fn extensionless_shebang_script_gets_text_plain() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("deploy script");   // space, no extension
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(get_mime_type(&script), mime::TEXT_PLAIN);
    }

    #[test]
    fn extensionless_pdf_bytes_get_application_pdf() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("report");
        std::fs::write(&pdf, b"%PDF-1.4\n%fake body").unwrap();
        assert_eq!(get_mime_type(&pdf).to_string(), "application/pdf");
    }

    #[test]
    fn a_txt_file_with_zip_magic_keeps_its_extension_mime() {
        // The sniff must only run on the fallback path - a known
        // extension wins even when the content lies.
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, b"PK\x03\x04 pretending to be a zip").unwrap();
        assert_eq!(get_mime_type(&file), mime::TEXT_PLAIN);
    }

    #[test]
    fn an_unreadable_extensionless_path_falls_back_to_text_plain() {
        // Sniff read errors must never fail the caller.
        assert_eq!(get_mime_type(Path::new("/no/such/file")), mime::TEXT_PLAIN);
    }
}
```

**Step 2:** `cargo test mime_tests` — FAIL.

**Step 3: Implement.** Priority order per the design: special-case list →
`mime_guess` → sniff only on the generic fallback; sniff order: shebang →
magic (`infer`) → mostly-printable-UTF-8 → give up (`None` → text/plain,
today's behaviour):

```rust
pub fn get_mime_type<P: AsRef<Path>>(path: P) -> Mime {
    let ext = path.as_ref().extension().and_then(|e| e.to_str());
    // Check the special extensions here (for types that mime_guess doesn't handle correctly)
    match ext {
        Some("ts") => return mime::TEXT_JAVASCRIPT,
        /* ... existing special cases unchanged ... */
        // No extension: the guess has nothing to work with - sniff.
        None => return sniffed(path.as_ref()).unwrap_or(mime::TEXT_PLAIN),
        _ => (),
    }
    // Otherwise just use mime_guess; sniff only when it has no real answer.
    match mime_guess::from_path(&path).first() {
        Some(mime) if mime != mime::APPLICATION_OCTET_STREAM => mime,
        _ => sniffed(path.as_ref()).unwrap_or(mime::TEXT_PLAIN),
    }
}

/// One bounded 256-byte read; any error skips the sniff (never fails
/// the caller). Only reached on the extension-fallback path.
fn sniffed(path: &Path) -> Option<Mime> {
    use std::io::Read;
    let mut head = [0u8; 256];
    let mut file = std::fs::File::open(path).ok()?;
    let n = file.read(&mut head).ok()?;
    sniff_mime(&head[..n])
}

/// Content sniff over the first bytes: shebang, then magic numbers
/// (infer: pdf/zip/gzip/sqlite/elf/png/jpeg/tar/...), then a
/// mostly-printable-UTF-8 heuristic.
fn sniff_mime(head: &[u8]) -> Option<Mime> {
    if head.is_empty() {
        return None;
    }
    if head.starts_with(b"#!") {
        return Some(mime::TEXT_PLAIN);
    }
    if let Some(kind) = infer::get(head) {
        return kind.mime_type().parse().ok();
    }
    // A 256-byte window may cut a multi-byte char: only judge the valid
    // prefix. Printable = no control bytes besides \n, \r, \t.
    let valid = match std::str::from_utf8(head) {
        Ok(s) => s,
        Err(e) if e.valid_up_to() > 0 => {
            std::str::from_utf8(&head[..e.valid_up_to()]).unwrap()
        }
        Err(_) => return None,
    };
    let printable = valid
        .chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'));
    printable.then(|| mime::TEXT_PLAIN)
}
```

Notes:
- `infer::get` on a tar head returns `application/x-tar` (checks the
  `ustar` magic at 257 — the 256-byte window misses it by one byte, so
  bump the buffer to 512 bytes if the tar case matters; do it: 512).
  Use `[0u8; 512]` and keep the doc comment honest.
- The opener (`open`, opener.rs:183) picks up the improvement for free —
  an extensionless script now routes to the text opener instead of
  `application`. No opener test asserts the old behaviour (verified).
- `get_mime_type` is also called per-draw in `util::print_metadata`
  (footer): the sniff only triggers for extensionless/unknown files and
  is one bounded read of an already-hot file — acceptable, matches the
  design's "keep it cheap".

**Step 4:** `cargo test mime_tests` — PASS. Also rerun the full suite:
the preview dispatch tests must still pass (extension-hit paths are
untouched).

**Step 5:** Commit: `feat(opener): content-sniff extensionless and octet-stream files`

---

### Task 10: Final wiring sweep + docs

**Files:**
- Modify: `src/panel/preview.rs` (audit `FilePreview::new` — every design
  arm now points at its wrapper: `native_image_preview`, `audio_preview`,
  `cert_preview` (native-first), `gz_preview`, `tar_preview`,
  `zip_preview`, `stat_preview`; the text/binary bat arms and the video
  arm are unchanged)
- Modify: `CLAUDE.md` (short "Architecture: native preview backends"
  section: native-first/shell-out-fallback pattern, the crates, the
  128-line cap convention, the gz tar-magic sniff, `sniff_mime` order in
  `get_mime_type`, and that `debug`-level "falling back" log lines are
  the diagnostic for a native backend failing — visible via the debug
  socket `log` command)

**Steps:**
1. Audit the dispatch match — no arm may still call `mediainfo` directly
   except video-without-ffmpeg and the explicit fallbacks.
2. Confirm no dead code: `unzip`'s inline invocation moved (Task 3),
   `cert_preview_external` reachable (Task 6), `tar_list` reachable as
   fallback (Task 4/5). `cargo clippy --all-targets` must be clean —
   clippy's dead-code lint is the check.
3. Write the CLAUDE.md section.
4. `cargo build && cargo test && cargo clippy --all-targets` — green.
5. Commit: `docs: native preview backends architecture notes`

---

### Task 11: E2E verification (tmux + debug socket)

No code changes — the CLAUDE.md interaction loop, driven against real
fixtures. House rule: fixture names contain spaces and `&`.

**Step 1: Build fixtures** (all tools here are fixture-generators only —
the previews under test must not need them):

```bash
cd <repo> && cargo build
FIXTURE=$(mktemp -d)
mkdir "$FIXTURE/Bilder & Videos"
printf 'hello from a plain text member\nsecond line\n' > "$FIXTURE/a file.txt"
zip -j "$FIXTURE/Bilder & Videos/archive with spaces.zip" "$FIXTURE/a file.txt"
tar -czf "$FIXTURE/Bilder & Videos/bundle of files.tar.gz" -C "$FIXTURE" "a file.txt"
gzip -k "$FIXTURE/a file.txt"          # -> "a file.txt.gz", a NON-tar gzip
openssl req -x509 -newkey rsa:2048 -keyout /dev/null \
  -out "$FIXTURE/server cert.pem" -days 3650 -nodes \
  -subj "/CN=rfm-e2e-test" 2>/dev/null
printf '#!/bin/sh\necho "sniffed as text"\n' > "$FIXTURE/deploy script"
chmod +x "$FIXTURE/deploy script"
# optional (needs ffmpeg): audio fixture
ffmpeg -f lavfi -i sine=frequency=440:duration=2 "$FIXTURE/tone with spaces.wav" 2>/dev/null
```

**Step 2: Launch** (isolated config not needed — previews are unconfigured):

```bash
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test \
  "./target/debug/rfm --debug-socket /tmp/rfm.sock $FIXTURE" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done
echo await-idle | socat - UNIX-CONNECT:/tmp/rfm.sock
```

**Step 3: Walk every fixture and check the preview pane.** Directories
sort first, so the initial selection is `Bilder & Videos`; use
`entries center` to confirm the order before asserting anything. For each
file: `tmux send-keys -t rfm-test j`, `await-idle`, then poll `state`
until `seq` stabilizes (preview decodes are async panel loads that
`await-idle` does not cover), then `tmux capture-pane -t rfm-test -p` and
grep:

| selection | expected in the preview pane |
|---|---|
| `Bilder & Videos` → enter (`l`), select zip | `a file.txt` member line |
| ...select `bundle of files.tar.gz` | `a file.txt` member line with mode string |
| back (`h`), select `a file.txt.gz` | `hello from a plain text member` — **the mis-dispatch fix** |
| `deploy script` (no extension) | its script text — **the sniff** |
| `server cert.pem` | `Subject:` line containing `rfm-e2e-test` |
| `tone with spaces.wav` (if built) | `Duration:` and `Sample rate:` lines |

**Step 4: Check the causal trail.**
`echo log | socat - UNIX-CONNECT:/tmp/rfm.sock` — with the debug socket
active, TRACE retention holds the full trail. There must be **no**
"falling back" / "trying unzip|tar|openssl|mediainfo" lines for these
fixtures (native paths handled everything), and no error lines.

**Step 5: Teardown** (the socket file survives the kill):

```bash
tmux kill-session -t rfm-test; rm -rf "$FIXTURE" /tmp/rfm.sock
```

No commit (or fold fixes found here into a `fix(preview):` commit with a
regression test per the TDD rule).

---

## Verification checklist (end state)

- [ ] `cargo build`, `cargo test`, `cargo clippy --all-targets` clean on
      rustc 1.83.0
- [ ] No preview arm spawns `unzip`/`openssl`, and `mediainfo`/`tar` only
      run on the fallback path (debug-socket `log` shows no fallback lines
      for healthy files)
- [ ] `foo.txt.gz` (non-tar gzip) previews its decompressed text
- [ ] Extensionless shebang script previews as text and opens via the
      text opener
- [ ] `.txt` file with zip magic still previews as text (sniff only on
      the fallback path)
- [ ] Corrupt zip/tar degrade to the shell-out path, then to error text —
      never a panic, never a blank preview
- [ ] Fixture paths with spaces and `&` work throughout
- [ ] `Cargo.lock` pins `ogg_pager 0.7.0` and `uuid 1.12.1` (MSRV guards)
