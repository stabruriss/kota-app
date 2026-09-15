use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENT_IMAGE_BLOCKS: usize = 16;
const MAX_JSONL_LINE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RETURNED_IMAGES: usize = 9;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeNativeImagesRequest {
    source_path: String,
    native_event_id: String,
    #[serde(default = "default_max_images")]
    max_images: usize,
}

fn default_max_images() -> usize {
    MAX_RETURNED_IMAGES
}

#[derive(Clone, Copy)]
struct ExtractionLimits {
    max_image_bytes: usize,
    max_event_image_blocks: usize,
    max_jsonl_line_bytes: usize,
    max_returned_images: usize,
}

const PRODUCTION_LIMITS: ExtractionLimits = ExtractionLimits {
    max_image_bytes: MAX_IMAGE_BYTES,
    max_event_image_blocks: MAX_EVENT_IMAGE_BLOCKS,
    max_jsonl_line_bytes: MAX_JSONL_LINE_BYTES,
    max_returned_images: MAX_RETURNED_IMAGES,
};

pub(crate) fn images_for_event(
    home_dir: &Path,
    request: &ClaudeNativeImagesRequest,
) -> BTreeMap<String, String> {
    extract_images_for_event(home_dir, request, PRODUCTION_LIMITS).unwrap_or_default()
}

fn extract_images_for_event(
    home_dir: &Path,
    request: &ClaudeNativeImagesRequest,
    limits: ExtractionLimits,
) -> Result<BTreeMap<String, String>, String> {
    let event_uuid = validated_event_uuid(&request.native_event_id)?;
    let source_path = validated_source_path(home_dir, &request.source_path)?;
    let max_images = request
        .max_images
        .min(limits.max_returned_images)
        .min(limits.max_event_image_blocks);
    if max_images == 0 {
        return Ok(BTreeMap::new());
    }

    let Some(event) = find_user_event(&source_path, &event_uuid, limits.max_jsonl_line_bytes)?
    else {
        return Ok(BTreeMap::new());
    };

    Ok(image_data_urls_from_event(&event, max_images, limits))
}

fn validated_event_uuid(native_event_id: &str) -> Result<String, String> {
    let trimmed = native_event_id.trim();
    let (uuid, block_index) = match trimmed.split_once(':') {
        Some((uuid, index)) if !index.is_empty() && !index.contains(':') => {
            index
                .parse::<u16>()
                .map_err(|_| "invalid native event block index".to_string())?;
            (uuid, Some(index))
        }
        Some(_) => return Err("invalid native event id".into()),
        None => (trimmed, None),
    };
    let _ = block_index;
    let parsed = Uuid::parse_str(uuid).map_err(|_| "invalid native event uuid".to_string())?;
    if uuid.len() != 36 || !uuid.is_ascii() {
        return Err("invalid native event uuid".into());
    }
    Ok(parsed.hyphenated().to_string())
}

fn validated_source_path(home_dir: &Path, source_path: &str) -> Result<PathBuf, String> {
    let source = PathBuf::from(source_path.trim());
    if source.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
        return Err("invalid Claude native log extension".into());
    }
    let projects_root = home_dir.join(".claude").join("projects");
    let canonical_root = fs::canonicalize(&projects_root)
        .map_err(|_| "Claude native log root unavailable".to_string())?;
    let canonical_source =
        fs::canonicalize(&source).map_err(|_| "Claude native log unavailable".to_string())?;
    if !canonical_source.starts_with(&canonical_root) || !canonical_source.is_file() {
        return Err("Claude native log outside allowed root".into());
    }
    Ok(canonical_source)
}

fn find_user_event(
    source_path: &Path,
    event_uuid: &str,
    max_line_bytes: usize,
) -> Result<Option<JsonValue>, String> {
    let file =
        fs::File::open(source_path).map_err(|error| format!("open Claude native log: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    while let Some(within_limit) = read_bounded_line(&mut reader, max_line_bytes, &mut line)
        .map_err(|error| format!("read Claude native log: {error}"))?
    {
        if !within_limit || line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<JsonValue>(&line) else {
            continue;
        };
        if event.get("uuid").and_then(JsonValue::as_str) != Some(event_uuid) {
            continue;
        }
        let is_user = event.get("type").and_then(JsonValue::as_str) == Some("user")
            && event
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(JsonValue::as_str)
                == Some("user");
        return Ok(is_user.then_some(event));
    }
    Ok(None)
}

/// Read one JSONL record without ever retaining more than `max_bytes` bytes.
/// Oversized records are consumed through their newline and reported as skipped.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
    output: &mut Vec<u8>,
) -> io::Result<Option<bool>> {
    output.clear();
    let mut oversized = false;
    let mut saw_bytes = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if saw_bytes {
                Ok(Some(!oversized))
            } else {
                Ok(None)
            };
        }
        saw_bytes = true;
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |index| index + 1);
        let record_bytes = newline.map_or(buffer, |index| &buffer[..index]);
        if !oversized {
            if output.len().saturating_add(record_bytes.len()) <= max_bytes {
                output.extend_from_slice(record_bytes);
            } else {
                oversized = true;
                output.clear();
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(!oversized));
        }
    }
}

fn image_data_urls_from_event(
    event: &JsonValue,
    max_images: usize,
    limits: ExtractionLimits,
) -> BTreeMap<String, String> {
    let Some(content) = event
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(JsonValue::as_array)
    else {
        return BTreeMap::new();
    };

    let marker_ids = text_marker_ids(content);
    if marker_ids.is_empty() {
        return BTreeMap::new();
    }
    let top_level_ids = event.get("imagePasteIds").and_then(JsonValue::as_array);
    let mut images = BTreeMap::new();
    let mut image_index = 0usize;

    for block in content {
        if block.get("type").and_then(JsonValue::as_str) != Some("image") {
            continue;
        }
        if image_index >= limits.max_event_image_blocks || images.len() >= max_images {
            break;
        }
        let marker_id = image_paste_id(block).or_else(|| {
            top_level_ids
                .and_then(|ids| ids.get(image_index))
                .and_then(small_marker_id)
        });
        image_index += 1;
        let Some(marker_id) = marker_id else {
            continue;
        };
        if !marker_ids.contains(&marker_id) || images.contains_key(&marker_id.to_string()) {
            continue;
        }
        let Some(data_url) = image_block_data_url(block, limits.max_image_bytes) else {
            continue;
        };
        images.insert(marker_id.to_string(), data_url);
    }
    images
}

fn text_marker_ids(content: &[JsonValue]) -> BTreeSet<u32> {
    let mut ids = BTreeSet::new();
    for text in content.iter().filter_map(|block| {
        (block.get("type").and_then(JsonValue::as_str) == Some("text"))
            .then(|| block.get("text").and_then(JsonValue::as_str))
            .flatten()
    }) {
        let mut rest = text;
        while let Some(start) = rest.find("[Image #") {
            rest = &rest[start + "[Image #".len()..];
            let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digit_count == 0 || rest.as_bytes().get(digit_count) != Some(&b']') {
                continue;
            }
            if let Ok(id) = rest[..digit_count].parse::<u32>() {
                ids.insert(id);
            }
            rest = &rest[digit_count + 1..];
        }
    }
    ids
}

fn image_paste_id(block: &JsonValue) -> Option<u32> {
    block
        .get("imagePasteId")
        .or_else(|| block.get("image_paste_id"))
        .and_then(small_marker_id)
}

fn small_marker_id(value: &JsonValue) -> Option<u32> {
    value
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .or_else(|| value.as_str()?.parse::<u32>().ok())
}

fn image_block_data_url(block: &JsonValue, max_image_bytes: usize) -> Option<String> {
    let source = block.get("source")?;
    if source.get("type").and_then(JsonValue::as_str) != Some("base64") {
        return None;
    }
    let mime = source.get("media_type").and_then(JsonValue::as_str)?;
    let data = source.get("data").and_then(JsonValue::as_str)?;
    let max_encoded_len = max_image_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4);
    if data.len() > max_encoded_len {
        return None;
    }
    let decoded = BASE64_STANDARD.decode(data).ok()?;
    if decoded.len() > max_image_bytes || !valid_image_signature(mime, &decoded) {
        return None;
    }
    Some(format!("data:{mime};base64,{data}"))
}

fn valid_image_signature(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);
    const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z6KAAAAAASUVORK5CYII=";

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        let ordinal = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "kota-claude-native-images-{}-{ordinal}",
            Uuid::new_v4().simple()
        ));
        let home = root.join("home");
        let source_dir = home.join(".claude/projects/project-agent");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("session.jsonl");
        (root, home, source)
    }

    fn request(source: &Path, event_id: &str) -> ClaudeNativeImagesRequest {
        ClaudeNativeImagesRequest {
            source_path: source.to_string_lossy().into_owned(),
            native_event_id: event_id.into(),
            max_images: MAX_RETURNED_IMAGES,
        }
    }

    fn event(uuid: &str, marker_text: &str, ids: Option<Vec<u32>>, images: Vec<&str>) -> JsonValue {
        let mut value = serde_json::json!({
            "type": "user",
            "uuid": uuid,
            "message": {
                "role": "user",
                "content": std::iter::once(serde_json::json!({
                    "type": "text",
                    "text": marker_text,
                })).chain(images.into_iter().map(|data| serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": data,
                    }
                }))).collect::<Vec<_>>()
            }
        });
        if let Some(ids) = ids {
            value["imagePasteIds"] = serde_json::json!(ids);
        }
        value
    }

    fn write_events(source: &Path, events: &[JsonValue]) {
        let text = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(source, format!("{text}\n")).unwrap();
    }

    #[test]
    fn extracts_real_claude_shape_using_authoritative_image_paste_ids() {
        let (root, home, source) = fixture();
        let top_level_uuid = Uuid::new_v4().hyphenated().to_string();
        let block_level_uuid = Uuid::new_v4().hyphenated().to_string();
        let mut block_level_event = event(
            &block_level_uuid,
            "[Image #71] [Image #72] compare",
            None,
            vec![PNG_BASE64, PNG_BASE64],
        );
        block_level_event["message"]["content"][1]["imagePasteId"] = serde_json::json!(71);
        block_level_event["message"]["content"][2]["imagePasteId"] = serde_json::json!(72);
        write_events(
            &source,
            &[
                event(
                    &top_level_uuid,
                    "[Image #61] [Image #62] compare",
                    Some(vec![61, 62]),
                    vec![PNG_BASE64, PNG_BASE64],
                ),
                block_level_event,
            ],
        );

        let images = images_for_event(&home, &request(&source, &format!("{top_level_uuid}:0")));
        let block_level_images = images_for_event(&home, &request(&source, &block_level_uuid));

        assert_eq!(images.len(), 2);
        assert_eq!(
            images.get("61").map(String::as_str),
            Some(format!("data:image/png;base64,{PNG_BASE64}").as_str())
        );
        assert!(images.contains_key("62"));
        assert_eq!(
            block_level_images.keys().cloned().collect::<Vec<_>>(),
            vec!["71", "72"]
        );
        assert!(!root.join("project-memory").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_unidentified_and_corrupt_blocks_without_guessing_by_order() {
        let (root, home, source) = fixture();
        let uuid_without_ids = Uuid::new_v4().hyphenated().to_string();
        let uuid_with_bad_first = Uuid::new_v4().hyphenated().to_string();
        write_events(
            &source,
            &[
                event(&uuid_without_ids, "[Image #1]", None, vec![PNG_BASE64]),
                event(
                    &uuid_with_bad_first,
                    "[Image #7] [Image #8]",
                    Some(vec![7, 8]),
                    vec!["not-base64", PNG_BASE64],
                ),
            ],
        );

        assert!(images_for_event(&home, &request(&source, &uuid_without_ids)).is_empty());
        let partial = images_for_event(&home, &request(&source, &uuid_with_bad_first));
        assert_eq!(partial.keys().cloned().collect::<Vec<_>>(), vec!["8"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_events_bad_ids_and_sources_outside_claude_projects() {
        let (root, home, source) = fixture();
        let uuid = Uuid::new_v4().hyphenated().to_string();
        write_events(
            &source,
            &[event(&uuid, "[Image #1]", Some(vec![1]), vec![PNG_BASE64])],
        );
        assert!(images_for_event(&home, &request(&source, "not-a-uuid")).is_empty());
        assert!(images_for_event(
            &home,
            &request(&source, &Uuid::new_v4().hyphenated().to_string()),
        )
        .is_empty());

        let outside = root.join("outside.jsonl");
        fs::copy(&source, &outside).unwrap();
        assert!(images_for_event(&home, &request(&outside, &uuid)).is_empty());
        assert!(!root.join("project-memory").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enforces_image_block_and_line_limits() {
        let (root, home, source) = fixture();
        let image_uuid = Uuid::new_v4().hyphenated().to_string();
        let line_uuid = Uuid::new_v4().hyphenated().to_string();
        write_events(
            &source,
            &[
                event(
                    &image_uuid,
                    "[Image #1] [Image #2] [Image #3]",
                    Some(vec![1, 2, 3]),
                    vec![PNG_BASE64, PNG_BASE64, PNG_BASE64],
                ),
                event(&line_uuid, "[Image #4]", Some(vec![4]), vec![PNG_BASE64]),
            ],
        );
        let tight = ExtractionLimits {
            max_image_bytes: BASE64_STANDARD.decode(PNG_BASE64).unwrap().len(),
            max_event_image_blocks: 2,
            max_jsonl_line_bytes: 1024,
            max_returned_images: 9,
        };
        let limited =
            extract_images_for_event(&home, &request(&source, &image_uuid), tight).unwrap();
        assert_eq!(limited.keys().cloned().collect::<Vec<_>>(), vec!["1", "2"]);

        let too_small_for_image = ExtractionLimits {
            max_image_bytes: 4,
            ..tight
        };
        assert!(extract_images_for_event(
            &home,
            &request(&source, &image_uuid),
            too_small_for_image,
        )
        .unwrap()
        .is_empty());

        let too_small_for_line = ExtractionLimits {
            max_jsonl_line_bytes: 32,
            ..tight
        };
        assert!(
            extract_images_for_event(&home, &request(&source, &line_uuid), too_small_for_line,)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_text_only_reference_never_becomes_an_image() {
        let (root, home, source) = fixture();
        let uuid = Uuid::new_v4().hyphenated().to_string();
        write_events(
            &source,
            &[serde_json::json!({
                "type": "user",
                "uuid": uuid,
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": "I only mentioned [Image #61]" }]
                },
                "imagePasteIds": []
            })],
        );

        assert!(images_for_event(&home, &request(&source, &uuid)).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
