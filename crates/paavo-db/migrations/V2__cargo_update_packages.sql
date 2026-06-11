-- Add `cargo_update_packages` (JSON array) to `job`. v1 jobs without
-- the column default to `[]`. The dispatch loop reads this and passes
-- it to `paavo_build::BuildPlan::cargo_update_packages`, which runs
-- `cargo update -p <pkg>` for each entry before `cargo build`. See
-- spec §7.2, §8.1, and §7.5 (nightly cron firing contract).
ALTER TABLE job
    ADD COLUMN cargo_update_packages TEXT NOT NULL DEFAULT '[]';
