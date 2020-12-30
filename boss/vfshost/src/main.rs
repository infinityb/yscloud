use std::collections::HashMap;

use futures::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio_postgres::{Client, NoTls};

use magnetite_common::{TorrentId, TorrentMeta};
use magnetite_vfs::{
    create_content_root, CreateInode, CreateInodeCommand, CreatePieceSha, CreatePieceShaCommand,
    CreateTorrentMeta, FetchDirectoriesQueryProcessor, FetchDirectoriesRequest, InodeType,
};

async fn load_torrent(
    cl: &mut Client,
    tm: &TorrentMeta,
    info_hash: &TorrentId,
) -> Result<(), failure::Error> {
    let prepped_create_piece_sha = CreatePieceShaCommand::prepare(&cl).await?;
    let prepped_create_inode = CreateInodeCommand::prepare(&cl).await?;

    let tx = cl.transaction().await?;

    let inode_root_directory = CreateInodeCommand::execute(
        &tx,
        &CreateInode {
            parent: None,
            file_name: "",
            file_type: InodeType::Root,
            global_offset: 0,
            file_length: 0,
        },
    )
    .await?;

    let info_hash_s = format!("{}", info_hash.hex());
    let meta_id = create_content_root(
        &tx,
        &CreateTorrentMeta {
            content_key: &info_hash_s[..],
            piece_length_shift: tm.info.piece_length_shift as i32,
            root_inode: inode_root_directory,
            total_length: tm.info.length as i64,
        },
    )
    .await?;

    let prepped_create_piece_sha_ptr = &prepped_create_piece_sha;
    let tx_ptr = &tx;
    let create_shas = stream::iter(tm.info.pieces.iter().enumerate().map(|(idx, piece)| {
        let piece = *piece;
        async move {
            let hexed = piece.hex().to_string();
            prepped_create_piece_sha_ptr
                .execute_prepared(
                    tx_ptr,
                    &CreatePieceSha {
                        torrent_meta_id: meta_id,
                        piece_index: idx as i64,
                        piece_sha: &hexed[..],
                    },
                )
                .await
        }
    }));

    create_shas.buffer_unordered(100).try_collect().await?;

    let mut inode_cache = HashMap::new();
    if tm.info.is_multi_file() {
        use std::os::unix::ffi::OsStrExt;

        for file in tm.info.files.iter() {
            let mut current_inode = inode_root_directory;

            // parser guarantees at least one component.
            let component_last_index = file.path.components().count() - 1;

            for (idx, c) in file.path.components().enumerate() {
                let path_component = std::str::from_utf8(c.as_os_str().as_bytes())?;

                if component_last_index == idx {
                    // last component is the file.

                    prepped_create_inode
                        .execute_prepared(
                            &tx,
                            &CreateInode {
                                parent: Some(current_inode),
                                file_name: path_component,
                                file_type: InodeType::Regular,
                                global_offset: file.global_offset as i64,
                                file_length: file.length as i64,
                            },
                        )
                        .await?;
                } else {
                    if let Some(next_inode) = inode_cache.get(&(current_inode, path_component)) {
                        current_inode = *next_inode;
                        continue;
                    }

                    let inode = prepped_create_inode
                        .execute_prepared(
                            &tx,
                            &CreateInode {
                                parent: Some(current_inode),
                                file_name: path_component,
                                file_type: InodeType::Directory,
                                global_offset: 0,
                                file_length: 0,
                            },
                        )
                        .await?;

                    inode_cache.insert((current_inode, path_component), inode);
                    current_inode = inode;
                }
            }
        }
    } else {
        //
    }

    // let tx_ptr = &tx;
    // let files = async move {
    //     // let create_shas = stream::iter(tm.info.pieces.iter().enumerate()
    //     //     .map(|(idx, piece)| {
    //     //         let piece = *piece;
    //     //         async move {
    //     //             let hexed = piece.hex().to_string();
    //     //             create_piece_sha(tx_ptr, &CreatePieceSha {
    //     //                 torrent_meta_id: meta_id,
    //     //                 piece_index: idx as i64,
    //     //                 piece_sha: &hexed[..],
    //     //             }).await
    //     //         }
    //     //     }));

    //     // create_shas.buffer_unordered(10).try_collect().await?;

    //     Result::<(), failure::Error>::Ok(())
    // };

    // let (r1, r2) = futures::join!(files, hashes);
    // r1?;
    // r2?;

    tx.commit().await?;
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TorrentInfoWrapper {
    info: bencode::Value,
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    use std::fs::File;
    use std::io::Read;

    let (mut client, connection) =
        tokio_postgres::connect("host=/tmp/psql dbname=magnetite", NoTls).await?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {}", e);
        }
    });

    if true {
        let prepped = FetchDirectoriesQueryProcessor::prepare(&client).await?;

        let tx = client.transaction().await?;
        let xx = prepped
            .execute_prepared(
                &tx,
                &FetchDirectoriesRequest {
                    root_inode: 1,
                    path: &["original", "0000"],
                },
            )
            .await?;
        drop(tx);

        let filenames = vec![
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/bofuri_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/happy-sugar-life_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/machikado-mazoku_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/made-in-abyss_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/kaguya-sama_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/mogra_2020-04-24.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/symphogear.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/gabriel-dropout_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/haifuri_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/keijo_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/demi-chan-wa-kataritai_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/senran-kagura.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/ssss-gridman.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/zombieland-saga.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/nanoha-movies.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/rike-koi.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/ping-pong.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/sabagebu.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/yuru-camp.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/garupan-das-finale.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/kizumonogatari.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/new-game.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/sansha-sanyou.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/ai-mai-mi_s03.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/stella-no-mahou.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/omnibus-2020-05-11.2.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/time-travel-shoujo.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/maidragon.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/ishuzoku-reviewers.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/shakunetsu-no-takkyuu-musume_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/shinkai-2020-05-27.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/houseki-no-kuni_s01_r2.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/monogatari_series_s01.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/monogatari_series_s02.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/monogatari_series_s03.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/monogatari_series_s04.torrent",
            "/Users/sell/dev/anime-piracy-empire/magnetite-config/yuuki-yuuna-wa-yuusha-de-aru_s01.torrent",
        ];
        for filename in filenames {
            let mut buf = Vec::new();
            let mut opened = File::open(&filename).unwrap();
            opened.read_to_end(&mut buf).unwrap();

            let tm: TorrentMeta = bencode::from_bytes(&buf[..]).unwrap();
            opened.read_to_end(&mut buf).unwrap();
            let ihw: TorrentInfoWrapper = bencode::from_bytes(&buf[..]).unwrap();
            let info_data = bencode::to_bytes(&ihw.info).unwrap();

            let mut hasher = Sha1::new();
            hasher.input(&info_data[..]);
            let mut tid = TorrentId::zero();
            tid.as_mut_bytes().copy_from_slice(&hasher.result()[..]);
            load_torrent(&mut client, &tm, &tid).await?;
            let tx = client.transaction().await?;
            tx.commit().await?;
        }
    }

    let prepped_fetch_dir = FetchDirectoriesQueryProcessor::prepare(&client).await?;
    let tx = client.transaction().await?;
    let dir = prepped_fetch_dir
        .execute_prepared(
            &tx,
            &FetchDirectoriesRequest {
                root_inode: 3693708,
                path: &["transcode-info"],
            },
        )
        .await?;
    tx.commit().await?;

    println!("{:#?}", dir);

    Ok(())
}
