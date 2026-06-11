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
fn board_subcommand_lists_all_verbs() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["board", "--help"])
        .assert()
        .success()
        .stdout(contains("add"))
        .stdout(contains("quarantine"))
        .stdout(contains("unquarantine"))
        .stdout(contains("remove"));
}

#[test]
fn board_remove_help_mentions_id_arg() {
    // Smoke-checks that `paavo-cli board remove --help` surfaces the
    // positional id arg — guards against accidental removal of the
    // BoardOp::Remove variant or its doc string.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["board", "remove", "--help"])
        .assert()
        .success()
        .stdout(contains("id"));
}
