-- Add `skip_cache` (boolean as INTEGER 0/1) to `job`. When 1, the
-- dispatcher always invokes `paavo_build::build_release` for this job
-- and ignores any `build_cache` hit on the same `tar_blake3`. v2 jobs
-- without the column default to 0 (cache-enabled, the historical
-- behaviour). Submitted via `--skip-cache` from paavo-cli (HTTP
-- request sets JobSpec::skip_cache=true); see spec §6.5 / §8.1.
ALTER TABLE job
    ADD COLUMN skip_cache INTEGER NOT NULL DEFAULT 0;
