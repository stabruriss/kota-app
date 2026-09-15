//! Opt-in measurements: read real configuration, mutate only temporary copies.
use super::*;
use std::fs;
use std::os::unix::fs::MetadataExt;

pub(super) fn cpu_ms() -> f64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) },
        0
    );
    value.tv_sec as f64 * 1000.0 + value.tv_nsec as f64 / 1_000_000.0
}

fn outputs(projects: &[PathBuf]) -> BTreeMap<PathBuf, (u64, u64)> {
    let mut out = BTreeMap::new();
    for root in projects {
        for entry in fs::read_dir(root.join(".agent-workspaces")).unwrap() {
            let cwd = entry.unwrap().path();
            for name in ["AGENTS.md", "CLAUDE.md", "missing-skills.txt"] {
                let path = cwd.join(name);
                if let Ok(metadata) = fs::metadata(&path) {
                    out.insert(path, (metadata.ino(), metadata.len()));
                }
            }
        }
    }
    out
}

#[test]
#[ignore = "manual performance measurement of temporary copies of local config"]
fn configured_project_sync_performance() {
    let temporary = std::env::temp_dir().join(format!("kota-sync-perf-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&temporary).unwrap();
    let mut projects = Vec::new();
    let mut agents = 0;
    for source in crate::registered_adapter_projects().unwrap() {
        let root = temporary.join(source.file_name().unwrap());
        fs::create_dir_all(root.join(".agent-workspaces")).unwrap();
        fs::create_dir(root.join("project-rules")).unwrap();
        if let Ok(entries) = fs::read_dir(crate::project_rules_dir(&source)) {
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_file()
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                {
                    fs::copy(
                        &path,
                        root.join("project-rules").join(path.file_name().unwrap()),
                    )
                    .unwrap();
                }
            }
        }
        for entry in fs::read_dir(source.join(".agent-workspaces")).unwrap() {
            let cwd = entry.unwrap().path();
            if !cwd.join("agent.yaml").is_file() {
                continue;
            }
            let copied = root
                .join(".agent-workspaces")
                .join(cwd.file_name().unwrap());
            fs::create_dir(&copied).unwrap();
            agents += 1;
            for name in ["agent.yaml", "SHELL.yaml", "AGENTS.md", "CLAUDE.md"] {
                let path = cwd.join(name);
                if path.is_file() {
                    fs::copy(&path, copied.join(name)).unwrap();
                }
            }
        }
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        projects.push(root);
    }
    let before = outputs(&projects);
    let start_cpu = cpu_ms();
    let start = Instant::now();
    for root in &projects {
        crate::regenerate_project_adapters_in_root(root).unwrap();
    }
    let unchanged_ms = start.elapsed().as_secs_f64() * 1000.0;
    let unchanged_cpu_ms = cpu_ms() - start_cpu;
    let after = outputs(&projects);
    assert_eq!(before, after);

    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    enqueue(Box::new(move || {
        entered.send(()).unwrap();
        gate.recv().unwrap();
    }));
    waiting.recv().unwrap();
    for n in 0..20 {
        for root in &projects {
            fs::write(
                root.join("project-rules/performance-always.md"),
                format!("# Performance fixture\nFinal value {n}\n"),
            )
            .unwrap();
            mark_project(root.clone(), "performance burst");
        }
    }
    let queued = worker().queue.lock().unwrap().tasks.len();
    assert_eq!(queued, projects.len());
    let start_cpu = cpu_ms();
    let start = Instant::now();
    release.send(()).unwrap();
    loop {
        // The receipt executes after any current sync has finished. Empty dirty
        // here therefore also means the final popped task completed its writes.
        let pending = call(|| Ok(!worker().queue.lock().unwrap().dirty.is_empty())).unwrap();
        if !pending {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let burst_ms = start.elapsed().as_secs_f64() * 1000.0;
    let burst_cpu_ms = cpu_ms() - start_cpu;
    let after = outputs(&projects);
    let changes: Vec<_> = after
        .iter()
        .filter(|(path, value)| before.get(*path).is_none_or(|old| old != *value))
        .collect();
    for (path, _) in &changes {
        if path
            .file_name()
            .is_some_and(|name| name == "AGENTS.md" || name == "CLAUDE.md")
        {
            assert!(fs::read_to_string(path).unwrap().contains("Final value 19"));
        }
    }
    println!(
        "{}",
        serde_json::json!({"benchmark":"project-sync-temporary-config-copies", "projects":projects.len(), "agents":agents, "unchanged":{"milliseconds":unchanged_ms,"cpu_ms":unchanged_cpu_ms,"writes":0}, "burst":{"marks":20*projects.len(),"queued_projects":queued,"milliseconds":burst_ms,"cpu_ms":burst_cpu_ms,"changed_outputs":changes.len(),"output_bytes":changes.iter().map(|(_,(_,size))| *size).sum::<u64>(),"final_value":"PASS"}})
    );
    fs::remove_dir_all(temporary).unwrap();
}
