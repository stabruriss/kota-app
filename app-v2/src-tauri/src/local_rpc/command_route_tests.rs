//! Compile the actual thin IPC dispatcher with fake sinks. No Tauri window,
//! CLI discovery, App credentials or Pi process is needed for this route check.
use std::{fs, process::Command};

#[test]
fn local_model_options_take_the_async_delegate_and_other_providers_keep_the_old_sink() {
    let source = include_str!("../lib.rs");
    let route = source
        .split("async fn provider_model_options_refresh(")
        .nth(1)
        .unwrap()
        .split("\nfn model(")
        .next()
        .unwrap();
    let code = format!("{HARNESS}\nasync fn provider_model_options_refresh({route}");
    let root = std::env::temp_dir().join(format!("local-route-fixture-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let input = root.join("route.rs");
    let output = root.join("route-test");
    fs::write(&input, code).unwrap();
    let build = Command::new("rustc")
        .args(["+1.91.0", "--edition=2021", "--test"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "route harness could not compile: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let result = Command::new(&output).output().unwrap();
    let _ = fs::remove_dir_all(root);
    assert!(result.status.success(), "actual dispatcher route failed");
}

const HARNESS: &str = r#"
struct AppHandle;
type SupportedProviderModel=String;
mod local_rpc {pub mod commands {
 pub async fn model_options(_: &crate::AppHandle)->Result<Vec<String>,String>{Ok(vec!["local-delegate".into()])}
}}
mod tauri {pub mod async_runtime {
 pub async fn spawn_blocking<T,F:FnOnce()->T>(f:F)->Result<T,String>{Ok(f())}
}}
fn provider_model_options_refresh_blocking(provider:&str)->Result<Vec<String>,String>{Ok(vec![format!("old-sink:{provider}")])}
fn complete<F:std::future::Future>(future:F)->F::Output{
 let mut future=std::pin::pin!(future);
 let mut context=std::task::Context::from_waker(std::task::Waker::noop());
 match future.as_mut().poll(&mut context){std::task::Poll::Ready(v)=>v,_=>panic!("fake sinks are immediately ready")}
}
#[test] fn routes(){
 assert_eq!(complete(provider_model_options_refresh(AppHandle,"kota-local".into())).unwrap(),["local-delegate"]);
 for provider in ["pi","claude","kota-local-extra"]{
  assert_eq!(complete(provider_model_options_refresh(AppHandle,provider.into())).unwrap(),[format!("old-sink:{provider}")]);
 }
}
"#;
