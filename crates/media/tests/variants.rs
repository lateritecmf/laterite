//! Variants: a preset renders once under a burst, is cached as a row and a
//! blob, and leaves with its record. Imports only the public API.

use laterite_core::Db;
use laterite_media::{
    gc, ingest, record, stream_of, variant, LocalDisk, NewMedia, NewMediaExtra, Preset,
    StorageDriver, VariantError, Variants,
};
use std::collections::HashMap;
use std::sync::Arc;

async fn db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_media::migrations::migrations()]).await
}

fn png(w: u32, h: u32) -> Vec<u8> {
    use image::{DynamicImage, ImageBuffer, Rgba};
    let img = ImageBuffer::from_fn(w, h, |x, _| Rgba([x as u8, 20, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

async fn stored(db: &Db, disk: &LocalDisk, bytes: Vec<u8>, name: &str) -> record::MediaRecord {
    let out = ingest(disk, stream_of(bytes), 1 << 24).await.unwrap();
    let id = record::create_with(
        db,
        NewMedia {
            disk: "public".into(),
            hash: out.hash,
            size: out.size as i64,
            content_type: out.content_type,
            original_name: name.into(),
            in_library: true,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap();
    record::find(db, id).await.unwrap().unwrap()
}

#[tokio::test]
async fn a_burst_renders_a_variant_once_and_a_text_file_has_none() {
    let (db, _g) = db().await;
    let dir = tempfile::tempdir().unwrap();
    let disk = Arc::new(LocalDisk::new(dir.path()));
    let photo = stored(&db, &disk, png(600, 300), "photo.png").await;
    let variants = Arc::new(Variants::new());
    let preset = Preset::new("card").width(300).height(300).crop();

    let mut tasks = Vec::new();
    for _ in 0..6 {
        let (db, disk, variants, photo, preset) = (
            db.clone(),
            Arc::clone(&disk),
            Arc::clone(&variants),
            photo.clone(),
            preset.clone(),
        );
        tasks.push(tokio::spawn(async move {
            variants
                .ensure(&db, disk.as_ref(), &photo, &preset)
                .await
                .unwrap()
        }));
    }
    let mut ids = Vec::new();
    for t in tasks {
        ids.push(t.await.unwrap().id);
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "one row for the burst");
    let made = variant::find(&db, photo.id, "card").await.unwrap().unwrap();
    assert_eq!((made.width, made.height), (Some(300), Some(300)));
    assert!(dir.path().join(&made.path).is_file());
    assert_eq!(variant::list_for(&db, photo.id).await.unwrap().len(), 1);

    let text = stored(&db, &disk, b"plain words".to_vec(), "notes.txt").await;
    assert!(matches!(
        variants.ensure(&db, disk.as_ref(), &text, &preset).await,
        Err(VariantError::NotImage)
    ));
}

#[tokio::test]
async fn gc_takes_a_records_variants_with_its_blob() {
    let (db, _g) = db().await;
    let dir = tempfile::tempdir().unwrap();
    let disk = Arc::new(LocalDisk::new(dir.path()));
    let photo = stored(&db, &disk, png(200, 100), "orphan.png").await;
    record::set_in_library(&db, photo.id, false, None)
        .await
        .unwrap();
    let variants = Variants::new();
    let made = variants
        .ensure(&db, disk.as_ref(), &photo, &Preset::new("thumb").width(50))
        .await
        .unwrap();
    assert!(dir.path().join(&made.path).is_file());
    let disks: HashMap<String, Arc<dyn StorageDriver>> =
        HashMap::from([("public".to_string(), disk.clone() as Arc<dyn StorageDriver>)]);
    let report = gc::collect(&db, &disks, chrono::Duration::seconds(-5))
        .await
        .unwrap();
    assert_eq!((report.records, report.blobs), (1, 1));
    assert!(
        !dir.path().join(&made.path).exists(),
        "the variant blob went too"
    );
    assert!(variant::list_for(&db, photo.id).await.unwrap().is_empty());
}
