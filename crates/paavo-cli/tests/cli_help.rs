use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn top_level_help_lists_all_subcommands() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("run"))
        .stdout(contains("new"))
        .stdout(contains("cancel"))
        .stdout(contains("logs"))
        .stdout(contains("jobs"))
        .stdout(contains("boards"))
        .stdout(contains("board"));
}

#[test]
fn board_subcommand_has_add_quarantine_unquarantine() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["board", "--help"])
        .assert()
        .success()
        .stdout(contains("add"))
        .stdout(contains("quarantine"))
        .stdout(contains("unquarantine"));
}
