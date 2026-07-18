-- Crash-safe ownership proof for executor partial files.
--
-- The token is generated randomly and committed before a partial is created.
-- A retry may only reclaim the exact token recorded on a RUNNING operation;
-- RUNNING by itself is deliberately not an ownership proof.
ALTER TABLE plan_operations
ADD COLUMN partial_lease_token TEXT
CHECK (
    partial_lease_token IS NULL OR (
        execution_state = 'RUNNING'
        AND operation_type IN (
            'COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED', 'COPY_TEMPORARY',
            'COPY_WITH_SUFFIX', 'PRESERVE_ACROSS_CONTEXT'
        )
        AND length(partial_lease_token) = 36
        AND substr(partial_lease_token, 9, 1) = '-'
        AND substr(partial_lease_token, 14, 1) = '-'
        AND substr(partial_lease_token, 19, 1) = '-'
        AND substr(partial_lease_token, 24, 1) = '-'
        AND lower(partial_lease_token) = partial_lease_token
        AND substr(partial_lease_token, 1, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_token, 10, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_token, 15, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_token, 20, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_token, 25, 12) NOT GLOB '*[^0-9a-f]*'
    )
);

-- Physical identity captured from the create_new handle and claimed only
-- after creation succeeds. The fixed-width hex encoding is
-- `<volume-serial>:<file-index>` (two u64 values).
ALTER TABLE plan_operations
ADD COLUMN partial_lease_identity TEXT
CHECK (
    partial_lease_identity IS NULL OR (
        partial_lease_token IS NOT NULL
        AND length(partial_lease_identity) = 33
        AND substr(partial_lease_identity, 17, 1) = ':'
        AND lower(partial_lease_identity) = partial_lease_identity
        AND substr(partial_lease_identity, 1, 16) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_identity, 18, 16) NOT GLOB '*[^0-9a-f]*'
        AND substr(partial_lease_identity, 18, 16) <> '0000000000000000'
    )
);
