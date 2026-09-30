-- Import-control v1 is deliberately separate from the 38 node tables.
-- Marker-first: even an empty table is refused by all normal store admission paths.
CREATE TABLE IF NOT EXISTS podmesh_import_state(
    singleton BIGINT NOT NULL,
    protocol_version BIGINT NOT NULL,
    migration_id VARCHAR(64) NOT NULL,
    plan_sha256 VARCHAR(64) NOT NULL,
    snapshot_sha256 VARCHAR(64) NOT NULL,
    source_manifest_sha256 VARCHAR(64) NOT NULL,
    source_schema_hex LONGTEXT NOT NULL,
    target_identity_sha256 VARCHAR(64) NOT NULL,
    phase VARCHAR(16) NOT NULL,
    serving BIGINT NOT NULL,
    ddl_step BIGINT NOT NULL,
    verification_root VARCHAR(64) NULL,
    PRIMARY KEY(singleton),
    CONSTRAINT import_singleton CHECK(singleton=1),
    CONSTRAINT import_protocol CHECK(protocol_version=1),
    CONSTRAINT import_non_serving CHECK(serving=0),
    CONSTRAINT import_phase CHECK(phase IN ('INCOMPLETE','COMPLETE'))
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_nopad_bin ROW_FORMAT=DYNAMIC;
CREATE TABLE IF NOT EXISTS podmesh_import_tables(
    table_name VARCHAR(128) NOT NULL,
    row_count BIGINT NOT NULL,
    canonical_sha256 VARCHAR(64) NOT NULL,
    PRIMARY KEY(table_name),
    CONSTRAINT import_nonnegative_rows CHECK(row_count>=0)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_nopad_bin ROW_FORMAT=DYNAMIC;
