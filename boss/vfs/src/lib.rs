use std::convert::{TryInto, TryFrom};
use std::collections::BTreeMap;
use std::fmt;

use serde::{Serialize, Deserialize};
use tokio_postgres::{Client, Statement, Transaction, Error};

// --

#[derive(Debug)]
pub struct NoEntityExists;

impl fmt::Display for NoEntityExists {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "NoEntityExists")
    }
}

impl std::error::Error for NoEntityExists {}

// --

#[derive(Debug)]
pub enum InodeType {
    Root,
    Regular,
    Directory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VfsEntry {
    pub inode: i64,
    // pub file_name: String,
    #[serde(flatten)]
    pub data: VfsEntryData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "file_type", rename_all="snake_case")] 
pub enum VfsEntryData {
    Regular(VfsEntryRegular),
    Directory(VfsEntryDirectory),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VfsEntryRegular {
    pub global_offset: i64,
    pub file_length: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VfsEntryDirectory {
    pub contents: BTreeMap<String, i64>,
    pub is_complete: bool,
}

pub struct FetchDirectoriesQueryProcessor(Statement);

#[derive(Debug)]
pub struct FetchDirectoriesRequest<'a> {
    pub root_inode: i64,
    pub path: &'a [&'a str],
}

#[derive(Debug)]
pub struct FetchDirectoriesResponse {
    pub inodes: BTreeMap<i64, VfsEntry>,
}

        // WITH RECURSIVE path AS (
        //     SELECT inode, 0 as depth FROM files files_base WHERE inode = 1
        //     UNION
        //         SELECT files_rec.inode, depth + 1 FROM files files_rec
        //         INNER JOIN path p ON files_rec.parent_inode = p.inode
        //         WHERE files_rec.file_name = ('{}'::text[])[depth + 1]
        // )
        // SELECT
        //     fi.inode,
        //     fi.parent_inode,
        //     fi.file_name,
        //     fi.file_type,
        //     fi.global_offset,
        //     fi.file_length
        // FROM files fi
        // INNER JOIN path p ON p.inode = fi.parent_inode;

impl FetchDirectoriesQueryProcessor {
    const QUERY: &'static str = "
        WITH RECURSIVE path AS (
            SELECT inode, 0 as depth FROM files files_base WHERE inode = $1
            UNION
                SELECT files_rec.inode, depth + 1 FROM files files_rec
                INNER JOIN path p ON files_rec.parent_inode = p.inode
                WHERE files_rec.file_name = ($2::text[])[depth + 1]
        )
        SELECT
            fi.inode,
            fi.parent_inode,
            fi.file_name,
            fi.file_type,
            fi.global_offset,
            fi.file_length
        FROM files fi
        INNER JOIN path p ON p.inode = fi.parent_inode;
    ";

    pub async fn prepare<'a>(tx: &'a Client) -> Result<Self, failure::Error> {
        let statement = tx.prepare(Self::QUERY).await?;
        Ok(FetchDirectoriesQueryProcessor(statement))
    }

    pub async fn execute<'a>(
        tx: &'a Transaction<'a>,
        req: &'a FetchDirectoriesRequest<'a>,
    ) -> Result<FetchDirectoriesResponse, failure::Error> {
        Self::execute_helper(Self::QUERY, tx, req).await
    }

    pub async fn execute_prepared<'a>(
        &self,
        tx: &'a Transaction<'a>,
        req: &'a FetchDirectoriesRequest<'a>,
    ) -> Result<FetchDirectoriesResponse, failure::Error> {
        Self::execute_helper(&self.0, tx, req).await
    }

    pub async fn execute_helper<'a, T: ?Sized>(
        stmt: &T,
        tx: &'a Transaction<'a>,
        req: &'a FetchDirectoriesRequest<'a>,
    ) -> Result<FetchDirectoriesResponse, failure::Error>
        where T: tokio_postgres::ToStatement
    {
        let rows = tx
            .query(stmt, &[&req.root_inode, &req.path])
            .await?;

        let mut inodes: BTreeMap<i64, VfsEntry> = BTreeMap::new();
        let mut directories: BTreeMap<i64, BTreeMap<String, i64>> = BTreeMap::new();

        let records = rows.iter().map(|row| FileRecord::try_from(row)).collect::<Result<Vec<FileRecord>, _>>()?;

        for fr in &records {
            let dir_data = directories.entry(fr.parent_inode).or_insert(Default::default());
            dir_data.insert(fr.file_name.clone(), fr.inode);

            if let FileType::Regular = fr.file_type {
                inodes.insert(fr.inode, VfsEntry {
                    inode: fr.inode,
                    data: VfsEntryData::Regular(VfsEntryRegular {
                        global_offset: fr.global_offset,
                        file_length: fr.file_length,
                    }),
                });
            }
            if let FileType::Directory = fr.file_type {
                inodes.insert(fr.inode, VfsEntry {
                    inode: fr.inode,
                    data: VfsEntryData::Directory(VfsEntryDirectory {
                        contents: Default::default(),
                        is_complete: false,
                    }),
                });
            }
        }
        for (k, contents) in directories {
            inodes.insert(k, VfsEntry {
                inode: k,
                data: VfsEntryData::Directory(VfsEntryDirectory {
                    contents,
                    is_complete: true,
                }),
            });
        }

        Ok(FetchDirectoriesResponse {
            inodes,
        })
    }
}

#[derive(Debug)]
struct FileRecord {
    inode: i64,
    parent_inode: i64,
    file_name: String,
    file_type: FileType,
    global_offset: i64,
    file_length: i64,
}

#[derive(Debug)]
enum FileType {
    Directory,
    Regular,
}

impl std::str::FromStr for FileType {
    type Err = failure::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "directory" => Ok(FileType::Directory),
            "regular" => Ok(FileType::Regular),
            _ => Err(failure::format_err!("unknown variant {:?}", s)),
        }
    }
}

impl TryFrom<&'_ tokio_postgres::Row> for FileRecord {
    type Error = failure::Error;

    fn try_from(row: &tokio_postgres::Row) -> Result<Self, Self::Error> {
        let file_type: FileType = row.try_get::<_, String>(3)?.parse()?;

        Ok(FileRecord {
            inode: row.try_get(0)?,
            parent_inode: row.try_get(1)?,
            file_name: row.try_get(2)?,
            file_type,
            global_offset: row.try_get(4)?,
            file_length: row.try_get(5)?,
        })
    }
}

// --

pub struct CreateInode<'a> {
    pub parent: Option<i64>,
    pub file_name: &'a str,
    pub file_type: InodeType,
    pub global_offset: i64,
    pub file_length: i64,
}

pub struct CreateInodeCommand(Statement);

impl CreateInodeCommand {
    const QUERY: &'static str = "
        INSERT INTO files
        VALUES (
            nextval('files_inode_seq'),
            $1, $2, $3, $4, $5
        ) RETURNING inode;
    ";

    pub async fn prepare<'a>(tx: &'a Client) -> Result<Self, failure::Error> {
        let statement = tx.prepare(CreateInodeCommand::QUERY).await?;
        Ok(CreateInodeCommand(statement))
    }

    pub async fn execute<'a>(tx: &'a Transaction<'a>, create: &'a CreateInode<'a>) -> Result<i64, failure::Error> {
        CreateInodeCommand::execute_helper(CreateInodeCommand::QUERY, tx, create).await
    }

    pub async fn execute_prepared<'a>(&self, tx: &'a Transaction<'a>, create: &'a CreateInode<'a>) -> Result<i64, failure::Error> {
        CreateInodeCommand::execute_helper(&self.0, tx, create).await
    }

    pub async fn execute_helper<'a, T: tokio_postgres::ToStatement + ?Sized>(stmt: &T, tx: &'a Transaction<'a>, create: &'a CreateInode<'a>) -> Result<i64, failure::Error> {
        let file_type = match create.file_type {
            InodeType::Regular => "regular",
            InodeType::Directory => "directory",
            InodeType::Root => "root",
        };

        let row = tx
            .query_one(stmt, &[
                &create.parent,
                &create.file_name,
                &file_type,
                &create.global_offset,
                &create.file_length,
            ])
            .await?;

        Ok(row.try_get(0)?)
    }
}

// --

pub struct FetchContentRootQuery<'a> {
    pub content_key: &'a str,
}

#[derive(Clone)]
pub struct FetchContentRootResponse {
    pub piece_length_shift: u8,
    pub total_length: i64,
    pub root_inode: i64,
}

pub struct FetchContentRootQueryProcessor(Statement);

impl FetchContentRootQueryProcessor {
    const QUERY: &'static str = "
        SELECT
            piece_length_shift,
            root_inode,
            total_length
        FROM torrent_meta
        WHERE content_key = $1;
    ";

    pub async fn prepare<'a>(tx: &'a Client) -> Result<Self, failure::Error> {
        let statement = tx.prepare(Self::QUERY).await?;
        Ok(Self(statement))
    }

    pub async fn execute<'a>(
        tx: &'a Transaction<'a>,
        create: &'a FetchContentRootQuery<'a>,
    ) -> Result<FetchContentRootResponse, failure::Error> {
        Self::execute_helper(Self::QUERY, tx, create).await
    }

    pub async fn execute_prepared<'a>(
        &self,
        tx: &'a Transaction<'a>,
        create: &'a FetchContentRootQuery<'a>,
    ) -> Result<FetchContentRootResponse, failure::Error> {
        Self::execute_helper(&self.0, tx, create).await
    }

    pub async fn execute_helper<'a, T: ?Sized>(
        stmt: &T,
        tx: &'a Transaction<'a>,
        create: &'a FetchContentRootQuery<'a>,
    ) -> Result<FetchContentRootResponse, failure::Error>
        where T: tokio_postgres::ToStatement,
    {
        let row = 
            tx.query_opt(stmt, &[&create.content_key]).await?
            .ok_or_else(|| NoEntityExists)?;

        let piece_length_shift: i32 = row.try_get(0)?;
        let piece_length_shift: u8 = piece_length_shift.try_into()?;
        
        Ok(FetchContentRootResponse {
            piece_length_shift,
            root_inode: row.try_get(1)?,
            total_length: row.try_get(2)?,
        })
    }
}

pub struct CreateTorrentMeta<'a> {
    pub content_key: &'a str,
    pub piece_length_shift: i32,
    pub root_inode: i64,
    pub total_length: i64,
}

pub async fn create_content_root<'a>(tx: &'a Transaction<'a>, create: &'a CreateTorrentMeta<'a>) -> Result<i64, Error> {
    const QUERY: &str = "
        INSERT INTO torrent_meta VALUES (
            nextval('torrent_meta_id_seq'),
            $1, $2, $3, $4
        ) RETURNING id
    ";
    
    let row = tx
        .query_one(QUERY, &[
            &create.content_key,
            &create.piece_length_shift,
            &create.root_inode,
            &create.total_length,
        ])
        .await?;

    row.try_get(0)
}

// --

pub struct CreatePieceSha<'a> {
    pub torrent_meta_id: i64,
    pub piece_index: i64,
    pub piece_sha: &'a str,
}

pub struct CreatePieceShaCommand(Statement);

impl CreatePieceShaCommand {
    const QUERY: &'static str = "INSERT INTO piece_hashes VALUES ($1, $2, $3);";

    pub async fn prepare<'a>(tx: &'a Client) -> Result<Self, failure::Error> {
        let statement = tx.prepare(CreatePieceShaCommand::QUERY).await?;
        Ok(CreatePieceShaCommand(statement))
    }

    pub async fn execute<'a>(tx: &'a Transaction<'a>, create: &'a CreatePieceSha<'a>) -> Result<(), failure::Error> {
        CreatePieceShaCommand::execute_helper(CreatePieceShaCommand::QUERY, tx, create).await
    }

    pub async fn execute_prepared<'a>(&self, tx: &'a Transaction<'a>, create: &'a CreatePieceSha<'a>) -> Result<(), failure::Error> {
        CreatePieceShaCommand::execute_helper(&self.0, tx, create).await
    }

    pub async fn execute_helper<'a, T: tokio_postgres::ToStatement + ?Sized>(stmt: &T, tx: &'a Transaction<'a>, create: &'a CreatePieceSha<'a>) -> Result<(), failure::Error> {
        tx
            .query(stmt, &[
                &create.torrent_meta_id,
                &create.piece_index,
                &create.piece_sha
            ])
            .await?;

        Ok(())
    }
}
