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
        .stdout(contains("board"))
        .stdout(contains("admin"));
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

#[test]
fn admin_subcommand_lists_purge() {
    // `paavo-cli admin --help` must list the `purge` op so operators
    // can find it. Guards against accidental removal of AdminOp::Purge.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["admin", "--help"])
        .assert()
        .success()
        .stdout(contains("purge"));
}

#[test]
fn admin_purge_help_mentions_wipe() {
    // The verb's --help text must telegraph that this is destructive
    // — operators reading `paavo-cli admin purge --help` need to see
    // something like "wipe"/"truncate"/"reset" so they don't run it
    // casually. We extract the actual help text and assert the
    // presence of at least one of those substrings; a future rewrite
    // that drops *all* of them should fail this test.
    let out = Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["admin", "purge", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    let has_destructive_hint = ["wipe", "truncate", "reset", "delete"]
        .iter()
        .any(|w| text.contains(w));
    assert!(
        has_destructive_hint,
        "`paavo-cli admin purge --help` should mention wipe/truncate/reset/delete \
         so operators know it is destructive; got:\n{text}"
    );
}

#[test]
fn admin_purge_help_lists_boards_and_yes() {
    // Operators reading `admin purge --help` must discover the new
    // opt-in board wipe and its confirmation bypass.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["admin", "purge", "--help"])
        .assert()
        .success()
        .stdout(contains("--boards"))
        .stdout(contains("--yes"));
}
