BEGIN;

DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS piece_hashes;
DROP TABLE IF EXISTS torrent_meta;

CREATE TABLE files (
    inode bigserial PRIMARY KEY,
    parent_inode bigint REFERENCES files(inode),
    file_name varchar NOT NULL,
    file_type varchar NOT NULL,
    global_offset bigint NOT NULL,
    file_length bigint NOT NULL,
    UNIQUE (parent_inode, file_name)

);

CREATE TABLE torrent_meta (
    id bigserial PRIMARY KEY,
    content_key varchar NOT NULL,
    piece_length_shift int NOT NULL,
    root_inode bigint NOT NULL,
    total_length bigint NOT NULL,
    UNIQUE (content_key)
);

CREATE TABLE piece_hashes (
    torrent_meta_id bigint REFERENCES torrent_meta(id),
    piece_index bigint NOT NULL,
    piece_sha varchar NOT NULL,
    PRIMARY KEY (torrent_meta_id, piece_index)
);

CLUSTER files_pkey ON files;
CLUSTER piece_hashes_pkey ON piece_hashes;

ALTER TABLE files OWNER TO "magnetite_vfs";
ALTER TABLE torrent_meta OWNER TO "magnetite_vfs";
ALTER TABLE piece_hashes OWNER TO "magnetite_vfs";
ALTER TABLE files_inode_seq OWNER TO "magnetite_vfs";
ALTER TABLE torrent_meta_id_seq OWNER TO "magnetite_vfs";

COMMIT;

-- INSERT INTO torrent_meta VALUES (
--     '6c11396879c4e1cf64bb2f2d1d9c6f14259fe655',
--     20,
--     1,
-- )



-- COMMIT;

