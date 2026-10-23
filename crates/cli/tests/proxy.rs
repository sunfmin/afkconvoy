//! The worker's terminal, rather than the fleet's environment, supplies its
//! inherited bypasses. Execute the generated line in a real shell too.
use std::process::Command;

use crate::common::{World, write};
use afkconvoy_core::{harness::Harness, proxy};

#[test]
fn the_worker_shell_extends_both_spellings_and_preserves_every_other_proxy() {
    for harness in Harness::ALL {
        let said = proxy::lists(&[], Harness::ClaudeCode).unwrap();
        let addition = proxy::addition(&said, harness);
        let command = proxy::line(
            "printf '%s\\n' \"$no_proxy\" \"$NO_PROXY\" \"$http_proxy\" \"$https_proxy\" \"$all_proxy\"",
            addition,
        );
        let output = Command::new("/bin/sh")
            .args(["-c", &command])
            .env("no_proxy", "localhost")
            .env("NO_PROXY", "internal")
            .env("http_proxy", "http://proxy:1")
            .env("https_proxy", "http://proxy:2")
            .env("all_proxy", "socks5://proxy:3")
            .output()
            .unwrap();
        assert!(output.status.success());
        let (lower, upper) = proxy::effective("localhost", "internal", addition);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{lower}\n{upper}\nhttp://proxy:1\nhttp://proxy:2\nsocks5://proxy:3\n")
        );
    }
    let literal = "a'$(exit 7);host";
    let output = Command::new("/bin/sh")
        .args(["-c", &proxy::line("printf '%s' \"$no_proxy\"", Some(literal))])
        .env_remove("no_proxy")
        .env_remove("NO_PROXY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), literal);
}

#[test]
fn plan_and_doctor_show_the_hosts_effective_per_harness_lists() {
    let world = World::new();
    for command in ["plan", "doctor"] {
        let answer = world
            .afk(&[command, "--now", "1000", "--worker-no-proxy", "codex=api.example,internal"])
            .ok();
        let lists = if command == "doctor" {
            &answer["host"]["worker_no_proxy"]
        } else {
            &answer["worker_no_proxy"]
        };
        let dsh = lists.as_array().unwrap().iter().find(|list| list["harness"] == "dsh").unwrap();
        assert_eq!(dsh["no_proxy"], "api.deepseek.com");
        let codex =
            lists.as_array().unwrap().iter().find(|list| list["harness"] == "codex").unwrap();
        assert_eq!(codex["no_proxy"], "api.example,internal");
        assert_eq!(codex["NO_PROXY"], codex["no_proxy"]);
    }
}

#[test]
fn dispatch_applies_the_host_list_without_putting_it_in_repo_config() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world
        .afk(&[
            "tick",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
            "--worker-command",
            "agent",
            "--worker-no-proxy",
            "direct.example",
        ])
        .ok();
    let orca = world.read_orca();
    let terminals = orca["terminals"].as_array().unwrap();
    assert!(
        terminals.iter().any(|t| t["command"].as_str().is_some_and(|line| line
            .contains("direct.example")
            && line.contains("export NO_PROXY="))),
        "{orca}"
    );
    write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nworker_no_proxy: {dsh: api.deepseek.com}\n```\n",
    );
    assert!(world.afk(&["config"]).error().contains("worker_no_proxy"));
}
