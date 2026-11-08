//! Test-only guest: real Wasm, no Core/production Node or OS API dependency.
use serde_json::{Value, json};
use std::sync::Mutex;

fn failure(code: &str) -> Value {
    json!({"apiVersion":1,"ok":false,"error":{"code":code,"message":"fixture refused"}})
}
fn handle(bytes: &[u8]) -> Vec<u8> {
    let request: Value = serde_json::from_slice(bytes).unwrap();
    let op = &request["operation"];
    let mode = op["config"]["mode"].as_str();
    let data = match op["method"].as_str().unwrap() {
        "describe" => {
            if request["node"]["kind"] == "fixture.wasm.hang" {
                loop {
                    std::hint::spin_loop();
                }
            }
            let mut definition = json!({"kind":if request["node"]["kind"]=="fixture.wasm.alt" {"fixture.wasm.alt"}else{"fixture.wasm"},"version":1,"inputs":[{"port":{"id":"in","dataType":{"id":"text-packet","version":1},"language":"ja-Latn","root":"text"},"required":true}],"outputs":[{"id":"out","dataType":{"id":"text-packet","version":1},"language":null,"root":"text"}],"configSchema":{"type":"object"},"defaultConfig":{"mode":"echo"}});
            // Separate chain fixture: preserve the declared input language.
            // Existing fixture.wasm/alt declarations remain unchanged.
            if matches!(
                request["node"]["kind"].as_str(),
                Some("fixture.wasm.chain" | "fixture.wasm.chain-alt")
            ) {
                definition["kind"] = request["node"]["kind"].clone();
                definition["outputs"][0]["language"] = json!("ja-Latn");
            }
            if request["node"]["kind"] == "fixture.wasm.duplicate" {
                definition["kind"] = request["node"]["kind"].clone();
                let wire =
                    serde_json::to_string(&json!({"apiVersion":1,"ok":true,"data":definition}))
                        .unwrap();
                return wire
                    .replace(
                        "\"defaultConfig\":{\"mode\":\"echo\"}",
                        "\"defaultConfig\":{\"mode\":\"echo\",\"mode\":\"echo\"}",
                    )
                    .into_bytes();
            }
            definition
        }
        "validate" => {
            if mode == Some("validateHold") {
                loop {
                    std::hint::spin_loop();
                }
            }
            if mode == Some("duplicateValidate") {
                return br#"{"apiVersion":1,"ok":true,"data":{"valid":true,"valid":true}}"#
                    .to_vec();
            }
            if !matches!(
                mode,
                Some(
                    "echo"
                        | "cpu"
                        | "echo2"
                        | "hold"
                        | "executeHold"
                        | "oversize"
                        | "sub/echo"
                        | "malformed"
                        | "forge"
                        | "noOutput"
                        | "duplicatePort"
                        | "duplicateEnvelope"
                        | "duplicateMeta"
                        | "duplicatePrepare"
                )
            ) && !mode.is_some_and(|s| s.starts_with("network"))
                || op["config"].as_object().unwrap().len() != 1
            {
                return serde_json::to_vec(&failure("invalid-config")).unwrap();
            }
            json!({"valid":true})
        }
        "prepare" => {
            if op.get("inputs").is_some() || op.get("mode").is_some() {
                return serde_json::to_vec(&failure("fixture-resource-projection")).unwrap();
            }
            let Some(reference) = mode else {
                return serde_json::to_vec(&failure("invalid-config")).unwrap();
            };
            let Some(resources) = op["resources"].as_object() else {
                return serde_json::to_vec(&failure("fixture-resource-missing")).unwrap();
            };
            if resources.len() != 1 || !resources.contains_key(reference) {
                return serde_json::to_vec(&failure("fixture-resource-scope")).unwrap();
            }
            let resource: Vec<u8> = serde_json::from_value(resources[reference].clone()).unwrap();
            if resource.starts_with(b"bad") {
                return serde_json::to_vec(&failure("fixture-resource-invalid")).unwrap();
            }
            if mode == Some("hold") {
                loop {
                    std::hint::spin_loop();
                }
            }
            if mode == Some("duplicatePrepare") {
                return br#"{"apiVersion":1,"ok":true,"data":{"valid":true,"prepared":true,"prepared":true}}"#.to_vec();
            }
            json!({"valid":true,"prepared":true})
        }
        "resume" => {
            if !mode.is_some_and(|s| s.starts_with("network"))
                || op.get("inputs").is_some()
                || request.get("runKey").is_some()
                || request.get("input").is_some()
            {
                return serde_json::to_vec(&failure("fixture-resume-projection")).unwrap();
            }
            if mode == Some("networkMany")
                || mode == Some("networkTwice") && op["continuation"]["hop"] == 1
            {
                return serde_json::to_vec(&json!({"apiVersion":1,"ok":true,"call":{"binding":"api","body":op["response"]["body"],"continuation":{"hop":2}}})).unwrap();
            }
            json!({"out":{"state":"data","packet":{"version":1,"root":{"type":"text","text":serde_json::to_string(&request).unwrap()}}}})
        }
        "execute" => {
            if mode.is_some_and(|s| s.starts_with("network")) {
                let text = op["inputs"]["in"]["packet"]["root"]["text"]
                    .as_str()
                    .unwrap();
                let mut call = json!({"binding":"api","body":text.as_bytes(),"continuation":{"text":text,"hop":1}});
                match mode {
                    Some("networkUnknown") => call["binding"] = "other".into(),
                    Some("networkOversizeBody") => call["body"] = json!(vec![120u8; 8192]),
                    Some("networkHugeContinuation") => {
                        call["continuation"] = json!("c".repeat(8192))
                    }
                    Some("networkBadBody") => call["body"] = json!([-1]),
                    Some("networkNoContinuation") => {
                        call.as_object_mut().unwrap().remove("continuation");
                    }
                    _ => (),
                }
                let mut reply = json!({"apiVersion":1,"ok":true,"call":call});
                if mode == Some("networkMixed") {
                    reply["data"] = json!({"out":{"state":"noOutput"}});
                }
                let bytes = serde_json::to_vec(&reply).unwrap();
                if mode == Some("networkDuplicate") {
                    return String::from_utf8(bytes)
                        .unwrap()
                        .replace(
                            "\"binding\":\"api\"",
                            "\"binding\":\"api\",\"binding\":\"api\"",
                        )
                        .into_bytes();
                }
                return bytes;
            }
            if matches!(mode, Some("hold" | "executeHold")) {
                loop {
                    std::hint::spin_loop();
                }
            }
            if mode == Some("cpu") {
                // Fixed synthetic work, not a production processing Node. The barrier
                // prevents the Rust compiler from replacing the loop with a formula.
                let mut sum = 0u64;
                for index in 0u64..100_000 {
                    let n = std::hint::black_box(index);
                    sum = sum.wrapping_add(n.wrapping_mul(n));
                }
                return serde_json::to_vec(&json!({"apiVersion":1,"ok":true,"data":{"out":{"state":"data","packet":{"version":1,"root":{"type":"text","text":sum.to_string()}}}}})).unwrap();
            }
            match mode {
                Some("duplicatePort") => return br#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"},"out":{"state":"noOutput"}}}"#.to_vec(),
                Some("duplicateEnvelope") => return br#"{"apiVersion":1,"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"}}}"#.to_vec(),
                Some("duplicateMeta") => return br#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"data","packet":{"version":1,"root":{"type":"text","text":"private-duplicate","meta":{"reading":"a","reading":"a"}}}}}}"#.to_vec(),
                _ => (),
            }
            if mode == Some("malformed") {
                return br#"{"apiVersion":1,"ok":true,"data":{"undeclared":{"state":"noOutput"}}}"#
                    .to_vec();
            }
            if mode == Some("forge") {
                return br#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"}},"physicalExit":true}"#.to_vec();
            }
            if mode == Some("noOutput") {
                json!({"out":{"state":"noOutput"}})
            } else {
                json!({"out":{"state":"data","packet":{"version":1,"root":{"type":"text","text":String::from_utf8(bytes.to_vec()).unwrap()}}}})
            }
        }
        _ => return serde_json::to_vec(&failure("unknown-operation")).unwrap(),
    };
    serde_json::to_vec(&json!({"apiVersion":1,"ok":true,"data":data})).unwrap()
}
static INPUT: Mutex<Option<Box<[u8]>>> = Mutex::new(None);
static REPLY: Mutex<Option<Box<[u8]>>> = Mutex::new(None);
#[unsafe(no_mangle)]
pub extern "C" fn diyime_api_version() -> i32 {
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn diyime_alloc(length: u32) -> u32 {
    if length == 0 || length > 8 * 1024 * 1024 {
        return 0;
    }
    let mut bytes = vec![0; length as usize].into_boxed_slice();
    let pointer = bytes.as_mut_ptr() as u32;
    *INPUT.lock().unwrap() = Some(bytes);
    pointer
}
#[unsafe(no_mangle)]
pub extern "C" fn diyime_call(pointer: u32, length: u32) -> u64 {
    let Some(input) = INPUT.lock().unwrap().take() else {
        return 0;
    };
    if input.as_ptr() as u32 != pointer || input.len() != length as usize {
        return 0;
    }
    let bytes = handle(&input).into_boxed_slice();
    let descriptor = ((bytes.len() as u64) << 32) | bytes.as_ptr() as u64;
    *REPLY.lock().unwrap() = Some(bytes);
    descriptor
}
#[unsafe(no_mangle)]
pub extern "C" fn diyime_release_reply() {
    REPLY.lock().unwrap().take();
}
