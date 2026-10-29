use super::*;
use crate::b64;
use std::io::Write;

fn ctx() -> CursorCtx {
    CursorCtx { col: 0, abs_row: 100, cols: 80, rows: 24, cell_w_px: 10, cell_h_px: 20, screen: Screen::Main }
}

fn run(p: &mut Protocol, s: &str) -> Vec<Action> {
    p.handle(s.as_bytes(), &ctx())
}

fn run_ctx(p: &mut Protocol, s: &str, c: &CursorCtx) -> Vec<Action> {
    p.handle(s.as_bytes(), c)
}

fn responses(a: &[Action]) -> Vec<String> {
    a.iter()
        .filter_map(|x| match x {
            Action::Respond(b) => Some(String::from_utf8(b.clone()).unwrap()),
            _ => None,
        })
        .collect()
}

fn only_response(a: &[Action]) -> String {
    let r = responses(a);
    assert_eq!(r.len(), 1, "expected exactly one response, got {a:?}");
    r.into_iter().next().unwrap()
}

fn rgba_b64(w: u32, h: u32) -> String {
    let mut v = Vec::new();
    for i in 0..(w * h) {
        v.extend_from_slice(&[(i % 251) as u8, 7, 9, 255]);
    }
    b64::encode(&v)
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut img = image::RgbaImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([x as u8, y as u8, 0x80, if x == 0 { 0 } else { 255 }]);
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn ok(id: u32) -> String {
    format!("\x1b_Gi={id};OK\x1b\\")
}

fn has_dirty(a: &[Action]) -> bool {
    a.contains(&Action::Dirty)
}

fn move_of(a: &[Action]) -> Option<(i32, i32)> {
    a.iter().find_map(|x| match x {
        Action::MoveCursor { dcols, drows } => Some((*dcols, *drows)),
        _ => None,
    })
}

// ------------------------------------------------------------ transmission

#[test]
fn transmit_rgba_direct() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=t,f=32,s=2,v=2,i=5;{}", rgba_b64(2, 2)));
    assert_eq!(only_response(&a), ok(5));
    let img = p.image(5).unwrap();
    assert_eq!((img.width, img.height, img.rgba.len()), (2, 2, 16));
    assert_eq!(img.rgba[1], 7);
    assert_eq!(p.take_dirty_images(), vec![5]);
    assert!(p.take_dirty_images().is_empty());
    assert_eq!(p.placement_count(), 0, "a=t must not display");
    assert!(move_of(&a).is_none());
}

#[test]
fn transmit_rgb_expands_alpha() {
    let mut p = Protocol::new();
    let data = b64::encode(&[1, 2, 3, 4, 5, 6]);
    let a = run(&mut p, &format!("f=24,s=2,v=1,i=1;{data}"));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(&p.image(1).unwrap().rgba[..], &[1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn default_format_is_rgba_and_default_action_transmit() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("s=1,v=1,i=9;{}", rgba_b64(1, 1)));
    assert_eq!(only_response(&a), ok(9));
    assert!(p.image(9).is_some());
}

#[test]
fn chunked_transmission_accumulates_and_only_first_has_keys() {
    let mut p = Protocol::new();
    let full = rgba_b64(4, 4); // 64 bytes -> 88 chars
    let (c1, rest) = full.split_at(40);
    let (c2, c3) = rest.split_at(24);
    assert!(run(&mut p, &format!("a=t,f=32,s=4,v=4,i=3,m=1;{c1}")).is_empty());
    assert!(run(&mut p, &format!("m=1;{c2}")).is_empty());
    assert!(p.image(3).is_none(), "nothing stored before final chunk");
    let a = run(&mut p, &format!("m=0;{c3}"));
    assert_eq!(only_response(&a), ok(3));
    let img = p.image(3).unwrap();
    assert_eq!((img.width, img.height), (4, 4));
    assert_eq!(b64::encode(&img.rgba), full);
}

#[test]
fn chunked_display_uses_cursor_of_final_chunk() {
    let mut p = Protocol::new();
    let full = rgba_b64(2, 2);
    let (c1, c2) = full.split_at(8);
    run(&mut p, &format!("a=T,f=32,s=2,v=2,i=4,m=1;{c1}"));
    let mut later = ctx();
    later.col = 7;
    later.abs_row = 555;
    let a = run_ctx(&mut p, &format!("m=0;{c2}"), &later);
    assert_eq!(only_response(&a), ok(4));
    let pl = p.placements_in(Screen::Main, 0..=1000).next().unwrap();
    assert_eq!((pl.col, pl.abs_row), (7, 555));
}

#[test]
fn chunk_quiet_on_last_chunk_applies() {
    let mut p = Protocol::new();
    let full = rgba_b64(1, 2);
    let (c1, c2) = full.split_at(4);
    run(&mut p, &format!("f=32,s=1,v=2,i=3,m=1;{c1}"));
    let a = run(&mut p, &format!("m=0,q=1;{c2}"));
    assert!(responses(&a).is_empty());
    assert!(p.image(3).is_some());
}

#[test]
fn partial_upload_aborted_by_delete_and_by_new_command() {
    let mut p = Protocol::new();
    let full = rgba_b64(1, 1);
    run(&mut p, &format!("f=32,s=1,v=1,i=3,m=1;{}", &full[..4]));
    run(&mut p, "a=d");
    let a = run(&mut p, &format!("m=0;{}", &full[4..]));
    // The stray final chunk is now a fresh command without dimensions.
    assert!(p.image(3).is_none());
    assert!(responses(&a).is_empty()); // no id => no response

    run(&mut p, &format!("f=32,s=1,v=1,i=6,m=1;{}", &full[..4]));
    let a = run(&mut p, &format!("a=t,f=32,s=1,v=1,i=7;{full}"));
    assert_eq!(only_response(&a), ok(7));
    assert!(p.image(6).is_none());
}

#[test]
fn oversize_chunked_upload_rejected() {
    let mut p = Protocol::with_memory_limit(1000);
    let chunk = "A".repeat(6000);
    let a = run(&mut p, &format!("f=32,s=10,v=10,i=1,m=1;{chunk}"));
    assert!(only_response(&a).contains("EFBIG"));
    // Aborted: a final chunk is not appended to anything.
    assert!(run(&mut p, "m=0;AAAA").is_empty());
}

#[test]
fn zlib_compressed_rgba() {
    let mut p = Protocol::new();
    let raw: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
    let data = b64::encode(&zlib(&raw));
    let a = run(&mut p, &format!("f=32,s=4,v=3,o=z,i=2;{data}"));
    assert_eq!(only_response(&a), ok(2));
    assert_eq!(&p.image(2).unwrap().rgba[..], &raw[..]);
}

#[test]
fn zlib_errors() {
    let mut p = Protocol::new();
    // Corrupt stream.
    let a = run(&mut p, &format!("f=32,s=1,v=1,o=z,i=2;{}", b64::encode(b"not zlib at all")));
    assert!(only_response(&a).starts_with("\x1b_Gi=2;EINVAL:"));
    // Decompressed data too short.
    let a = run(&mut p, &format!("f=32,s=2,v=2,o=z,i=2;{}", b64::encode(&zlib(&[0; 4]))));
    assert!(only_response(&a).contains("ENODATA"));
    // Decompressed data too long (bomb protection).
    let a = run(&mut p, &format!("f=32,s=1,v=1,o=z,i=2;{}", b64::encode(&zlib(&[0; 4096]))));
    assert!(only_response(&a).contains("EFBIG"));
    assert!(p.image(2).is_none());
}

#[test]
fn png_transmission() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("f=100,i=8;{}", b64::encode(&png_bytes(5, 3))));
    assert_eq!(only_response(&a), ok(8));
    let img = p.image(8).unwrap();
    assert_eq!((img.width, img.height), (5, 3));
    assert_eq!(img.rgba.len(), 5 * 3 * 4);
    assert_eq!(img.rgba[3], 0, "alpha preserved");
    assert_eq!(img.rgba[4 + 3], 255);
}

#[test]
fn png_with_zlib() {
    let mut p = Protocol::new();
    let png = png_bytes(4, 4);
    let a = run(&mut p, &format!("f=100,o=z,S={},i=1;{}", png.len(), b64::encode(&zlib(&png))));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(p.image(1).unwrap().width, 4);
}

#[test]
fn bad_png_is_error() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("f=100,i=1;{}", b64::encode(b"definitely not a png")));
    assert!(only_response(&a).contains("EINVAL"));
    let a = run(&mut p, "f=100,i=1;");
    assert!(only_response(&a).contains("ENODATA"));
}

#[test]
fn error_codes() {
    let mut p = Protocol::new();
    // Missing dimensions.
    let a = run(&mut p, &format!("f=32,i=1;{}", rgba_b64(1, 1)));
    assert!(only_response(&a).contains("EINVAL"));
    // Short data.
    let a = run(&mut p, &format!("f=32,s=2,v=2,i=1;{}", rgba_b64(1, 1)));
    let r = only_response(&a);
    assert!(r.contains("ENODATA") && r.contains("4 < 16"), "{r}");
    // Too much data.
    let a = run(&mut p, &format!("f=32,s=1,v=1,i=1;{}", rgba_b64(2, 2)));
    assert!(only_response(&a).contains("EFBIG"));
    // Bad base64.
    let a = run(&mut p, "f=32,s=1,v=1,i=1;!!!!");
    assert!(only_response(&a).contains("EINVAL"));
    // Unsupported format / medium.
    let a = run(&mut p, "f=77,s=1,v=1,i=1;AAAA");
    assert!(only_response(&a).contains("EINVAL"));
    let a = run(&mut p, "f=16,s=1,v=1,i=1;AAAA");
    assert!(only_response(&a).contains("EINVAL"));
    // Both i and I.
    let a = run(&mut p, &format!("f=32,s=1,v=1,i=1,I=2;{}", rgba_b64(1, 1)));
    assert!(only_response(&a).contains("EINVAL"));
    // Malformed control data still answers with the id.
    let a = run(&mut p, "i=4,z=abc;AAAA");
    assert!(only_response(&a).starts_with("\x1b_Gi=4;EINVAL"));
    // Unknown action.
    let a = run(&mut p, "a=Z,i=4");
    assert!(only_response(&a).contains("EINVAL"));
    // Put of unknown image.
    let a = run(&mut p, "a=p,i=99");
    assert_eq!(only_response(&a), "\x1b_Gi=99;ENOENT:Image not found\x1b\\");
    let a = run(&mut p, "a=p,I=99");
    assert!(only_response(&a).starts_with("\x1b_GI=99;ENOENT"));
    assert_eq!(p.image_count(), 0);
}

#[test]
fn size_limit_rejects_huge_images() {
    let mut p = Protocol::new();
    let a = run(&mut p, "f=32,s=10001,v=10,i=1;AAAA");
    assert!(only_response(&a).contains("EFBIG"));
    let a = run(&mut p, "f=24,s=10,v=10001,i=1;AAAA");
    assert!(only_response(&a).contains("EFBIG"));
}

#[test]
fn huge_dimensions_within_limit_but_over_quota_are_enospc() {
    let mut p = Protocol::with_memory_limit(1024);
    let a = run(&mut p, "f=32,s=100,v=100,i=1;AAAA");
    assert!(only_response(&a).contains("ENOSPC"));
}

#[test]
fn png_dimension_limit() {
    // 10001x1 PNG is small on disk but over the dimension limit.
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("f=100,i=1;{}", b64::encode(&png_bytes(10001, 1))));
    assert!(only_response(&a).contains("EFBIG"));
    assert!(p.image(1).is_none());
}

// ------------------------------------------------------------------- ids/q

#[test]
fn no_id_no_response() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=T,f=32,s=1,v=1;{}", rgba_b64(1, 1)));
    assert!(responses(&a).is_empty());
    assert_eq!(p.image_count(), 1);
    assert_eq!(p.placement_count(), 1);
    // Errors are also silent without an id.
    assert!(responses(&run(&mut p, "f=32,s=2,v=2;AAAA")).is_empty());
}

#[test]
fn image_number_allocates_id_and_responds_with_both() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("I=13,f=32,s=1,v=1;{}", rgba_b64(1, 1)));
    let r = only_response(&a);
    assert!(r.starts_with("\x1b_Gi=") && r.contains(",I=13;OK"), "{r}");
    let id: u32 = r["\x1b_Gi=".len()..].split(',').next().unwrap().parse().unwrap();
    assert!(id != 0);
    assert_eq!(p.image(id).unwrap().number, 13);
    // Same number again creates a second image, and put targets the newest.
    let a2 = run(&mut p, &format!("I=13,f=32,s=1,v=1;{}", rgba_b64(1, 1)));
    let r2 = only_response(&a2);
    let id2: u32 = r2["\x1b_Gi=".len()..].split(',').next().unwrap().parse().unwrap();
    assert_ne!(id, id2);
    let put = run(&mut p, "a=p,I=13,p=4");
    assert_eq!(only_response(&put), format!("\x1b_Gi={id2},I=13,p=4;OK\x1b\\"));
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap().image_id, id2);
}

#[test]
fn quiet_levels() {
    let mut p = Protocol::new();
    let img = rgba_b64(1, 1);
    assert!(responses(&run(&mut p, &format!("i=1,q=1,s=1,v=1;{img}"))).is_empty());
    // q=1 still reports errors.
    assert!(only_response(&run(&mut p, "i=1,q=1,s=1,v=1;AAAA")).contains("ENODATA"));
    // q=2 suppresses everything.
    assert!(responses(&run(&mut p, "i=1,q=2,s=1,v=1;AAAA")).is_empty());
    assert!(responses(&run(&mut p, &format!("i=1,q=2,s=1,v=1;{img}"))).is_empty());
    assert!(responses(&run(&mut p, "a=p,i=1,q=1")).is_empty());
    assert!(responses(&run(&mut p, "a=p,i=50,q=2")).is_empty());
    assert!(only_response(&run(&mut p, "a=p,i=50,q=1")).contains("ENOENT"));
}

#[test]
fn replacing_image_id_drops_old_placements() {
    let mut p = Protocol::new();
    let img = rgba_b64(1, 1);
    run(&mut p, &format!("a=T,i=1,s=1,v=1;{img}"));
    assert_eq!(p.placement_count(), 1);
    let v1 = p.image(1).unwrap().texture_version;
    p.take_dirty_images();
    let a = run(&mut p, &format!("a=t,i=1,s=1,v=1;{img}"));
    assert!(has_dirty(&a), "removing a visible placement dirties the screen");
    assert_eq!(p.placement_count(), 0);
    assert_ne!(p.image(1).unwrap().texture_version, v1);
    assert_eq!(p.take_dirty_images(), vec![1]);
}

// -------------------------------------------------------------------- query

#[test]
fn query_validates_without_storing() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=q,i=31,s=1,v=1,f=24;{}", b64::encode(&[1, 2, 3])));
    assert_eq!(only_response(&a), ok(31));
    assert_eq!(p.image_count(), 0);
    assert_eq!(p.memory_used(), 0);
    assert!(p.take_dirty_images().is_empty());
    let a = run(&mut p, "a=q,i=31,s=1,v=1,f=32;AAAA");
    assert!(only_response(&a).contains("ENODATA"));
}

#[test]
fn query_does_not_replace_existing_image() {
    let mut p = Protocol::new();
    run(&mut p, &format!("i=31,s=1,v=1;{}", rgba_b64(1, 1)));
    let before = p.image(31).unwrap().texture_version;
    run(&mut p, &format!("a=q,i=31,s=2,v=1;{}", rgba_b64(2, 1)));
    assert_eq!(p.image(31).unwrap().texture_version, before);
    assert_eq!(p.image(31).unwrap().width, 1);
}

#[test]
fn feature_query() {
    let mut p = Protocol::new();
    let a = run(&mut p, "a=q,i=31,e=T,U=1,C=1");
    assert_eq!(only_response(&a), "\x1b_Gi=31,e=T,C=1,U=1;OK\x1b\\");
    let a = run(&mut p, "a=q,i=32,e=f,r=1,X=1");
    assert_eq!(only_response(&a), "\x1b_Gi=32;OK\x1b\\");
}

#[test]
fn feature_query_does_not_disturb_chunked_upload() {
    let mut p = Protocol::new();
    let full = rgba_b64(1, 2);
    run(&mut p, &format!("f=32,s=1,v=2,i=3,m=1;{}", &full[..4]));
    run(&mut p, "a=q,i=31,e=T");
    let a = run(&mut p, &format!("m=0;{}", &full[4..]));
    assert_eq!(only_response(&a), ok(3));
}

#[test]
fn animation_is_politely_rejected() {
    let mut p = Protocol::new();
    let a = run(&mut p, "a=a,i=1,s=3");
    assert!(only_response(&a).contains("EINVAL"));
    assert!(responses(&run(&mut p, "a=a,s=3")).is_empty());
}

// ---------------------------------------------------------------- placement

fn setup(p: &mut Protocol, id: u32, w: u32, h: u32) {
    let a = run(p, &format!("i={id},s={w},v={h},f=32,q=2;{}", rgba_b64(w, h)));
    assert!(a.is_empty());
}

#[test]
fn put_by_id_displays_at_cursor_and_replies() {
    let mut p = Protocol::new();
    setup(&mut p, 10, 30, 40); // 3 cols x 2 rows at 10x20
    let mut c = ctx();
    c.col = 5;
    c.abs_row = 200;
    let a = run_ctx(&mut p, "a=p,i=10,p=7", &c);
    assert_eq!(only_response(&a), "\x1b_Gi=10,p=7;OK\x1b\\");
    assert!(has_dirty(&a));
    let pl: Vec<_> = p.placements_in(Screen::Main, 200..=200).collect();
    assert_eq!(pl.len(), 1);
    let pl = pl[0];
    assert_eq!((pl.image_id, pl.placement_id, pl.col, pl.abs_row), (10, 7, 5, 200));
    assert_eq!((pl.cols, pl.rows), (3, 2));
    assert_eq!(pl.src_rect, Rect { x: 0, y: 0, w: 30, h: 40 });
}

#[test]
fn put_same_placement_id_replaces_and_zero_id_accumulates() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 20);
    run(&mut p, "a=p,i=1,p=5");
    let mut c = ctx();
    c.col = 9;
    run_ctx(&mut p, "a=p,i=1,p=5", &c);
    assert_eq!(p.placement_count(), 1);
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap().col, 9);
    run(&mut p, "a=p,i=1");
    run(&mut p, "a=p,i=1");
    assert_eq!(p.placement_count(), 3);
}

#[test]
fn transmit_and_display_replies_once() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=T,i=2,p=3,s=1,v=1;{}", rgba_b64(1, 1)));
    assert_eq!(only_response(&a), "\x1b_Gi=2,p=3;OK\x1b\\");
    assert_eq!(p.placement_count(), 1);
}

#[test]
fn placement_id_ignored_for_anonymous_image() {
    let mut p = Protocol::new();
    run(&mut p, &format!("a=T,p=3,s=1,v=1;{}", rgba_b64(1, 1)));
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap().placement_id, 0);
}

#[test]
fn cell_size_from_pixels() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 25, 41); // ceil(25/10)=3, ceil(41/20)=3
    run(&mut p, "a=p,i=1");
    let pl = p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap().clone();
    assert_eq!((pl.cols, pl.rows), (3, 3));
}

#[test]
fn scaling_with_c_and_r() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 100, 100);
    run(&mut p, "a=p,i=1,c=4,r=6,p=1");
    run(&mut p, "a=p,i=1,c=10,p=2"); // 100px wide cells, square image => 100px high => 5 rows
    run(&mut p, "a=p,i=1,r=3,p=3"); // 60px high => 60px wide => 6 cols
    let get = |pid| {
        p.placements_in(Screen::Main, 0..=u64::MAX)
            .find(|x| x.placement_id == pid)
            .map(|x| (x.cols, x.rows))
            .unwrap()
    };
    assert_eq!(get(1), (4, 6));
    assert_eq!(get(2), (10, 5));
    assert_eq!(get(3), (6, 3));
}

#[test]
fn dest_px_follows_native_size_and_c_r() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 100, 50);
    run(&mut p, "a=p,i=1,p=1"); // native
    run(&mut p, "a=p,i=1,c=4,r=3,p=2"); // stretched to the cells (10x20 px each)
    run(&mut p, "a=p,i=1,c=5,p=3"); // 50px wide => 25px high
    run(&mut p, "a=p,i=1,r=2,p=4"); // 40px high => 80px wide
    let get = |pid| {
        p.placements_in(Screen::Main, 0..=u64::MAX)
            .find(|x| x.placement_id == pid)
            .map(|x| x.dest_px)
            .unwrap()
    };
    assert_eq!(get(1), (100, 50));
    assert_eq!(get(2), (40, 60));
    assert_eq!(get(3), (50, 25));
    assert_eq!(get(4), (80, 40));
}

#[test]
fn source_rect_is_clamped_and_affects_size() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 100, 100);
    run(&mut p, "a=p,i=1,x=10,y=20,w=50,h=40,p=1");
    run(&mut p, "a=p,i=1,x=90,y=90,w=500,h=500,p=2");
    run(&mut p, "a=p,i=1,x=50,p=3");
    let get = |pid| p.placements_in(Screen::Main, 0..=u64::MAX).find(|x| x.placement_id == pid).cloned().unwrap();
    let a = get(1);
    assert_eq!(a.src_rect, Rect { x: 10, y: 20, w: 50, h: 40 });
    assert_eq!((a.cols, a.rows), (5, 2));
    assert_eq!(get(2).src_rect, Rect { x: 90, y: 90, w: 10, h: 10 });
    assert_eq!(get(3).src_rect, Rect { x: 50, y: 0, w: 50, h: 100 });
    // Entirely outside.
    let r = run(&mut p, "a=p,i=1,x=100,p=9");
    assert!(only_response(&r).contains("EINVAL"));
}

#[test]
fn pixel_offsets_extend_cell_span_and_are_clamped() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 20); // exactly 1x1 cell
    run(&mut p, "a=p,i=1,X=3,Y=5,p=1");
    run(&mut p, "a=p,i=1,X=999,Y=999,p=2");
    let get = |pid| p.placements_in(Screen::Main, 0..=u64::MAX).find(|x| x.placement_id == pid).cloned().unwrap();
    assert_eq!(get(1).pixel_offset, (3, 5));
    assert_eq!((get(1).cols, get(1).rows), (2, 2));
    assert_eq!(get(2).pixel_offset, (9, 19));
}

#[test]
fn zero_cell_size_uses_fallback() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 16, 32);
    let mut c = ctx();
    c.cell_w_px = 0;
    c.cell_h_px = 0;
    run_ctx(&mut p, "a=p,i=1", &c);
    let pl = p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap();
    assert_eq!((pl.cols, pl.rows), (2, 2));
}

#[test]
fn z_index_is_recorded() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 20);
    run(&mut p, "a=p,i=1,z=-5,p=1");
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).next().unwrap().z, -5);
}

#[test]
fn cursor_movement_policy() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 30, 60); // 3 cols x 3 rows
    let a = run(&mut p, "a=p,i=1");
    assert_eq!(move_of(&a), Some((3, 2)), "cursor goes after last column on the last row");
    let a = run(&mut p, "a=p,i=1,C=1");
    assert_eq!(move_of(&a), None);
    // Single row image: only horizontal movement.
    setup(&mut p, 2, 30, 20);
    assert_eq!(move_of(&run(&mut p, "a=p,i=2")), Some((3, 0)));
    // a=t never moves the cursor, a=T does.
    let a = run(&mut p, &format!("a=T,i=3,s=1,v=1;{}", rgba_b64(1, 1)));
    assert_eq!(move_of(&a), Some((1, 0)));
    let a = run(&mut p, &format!("a=T,i=4,C=1,s=1,v=1;{}", rgba_b64(1, 1)));
    assert_eq!(move_of(&a), None);
}

#[test]
fn cursor_movement_is_clamped_to_screen_width() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 1000, 20); // 100 cols
    let a = run(&mut p, "a=p,i=1");
    assert_eq!(move_of(&a), Some((79, 0)));
    let mut c = ctx();
    c.col = 79;
    let a = run_ctx(&mut p, "a=p,i=1", &c);
    assert_eq!(move_of(&a), None);
}

#[test]
fn virtual_placements_are_stored_with_flag_and_do_not_move_cursor() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 100, 100);
    let a = run(&mut p, "a=p,i=1,U=1,c=4,r=2,p=9");
    assert_eq!(only_response(&a), "\x1b_Gi=1,p=9;OK\x1b\\");
    assert_eq!(move_of(&a), None);
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).count(), 0);
    let v: Vec<_> = p.virtual_placements(Screen::Main).collect();
    assert_eq!(v.len(), 1);
    assert!(v[0].is_virtual);
    assert_eq!((v[0].cols, v[0].rows), (4, 2));
    // Position-based deletes never touch virtual placements.
    run(&mut p, "a=d,d=z,z=0");
    run(&mut p, "a=d,d=c");
    assert_eq!(p.virtual_placements(Screen::Main).count(), 1);
    // d=i does.
    run(&mut p, "a=d,d=i,i=1");
    assert_eq!(p.virtual_placements(Screen::Main).count(), 0);
    // So does delete-all (which is also what ED 2 / RIS send), for both cases.
    for d in ["a", "A"] {
        run(&mut p, "a=p,i=1,U=1,c=4,r=2,p=9");
        assert_eq!(p.virtual_placements(Screen::Main).count(), 1);
        run(&mut p, &format!("a=d,d={d}"));
        assert_eq!(p.virtual_placements(Screen::Main).count(), 0, "d={d}");
        setup(&mut p, 1, 100, 100);
    }
}

#[test]
fn transmit_and_virtual_place_in_one_command() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=T,U=1,i=5,c=3,r=1,q=2,s=1,v=1;{}", rgba_b64(1, 1)));
    assert!(responses(&a).is_empty());
    assert_eq!(p.virtual_placements(Screen::Main).count(), 1);
}

#[test]
fn relative_placement_not_supported() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 20);
    assert!(only_response(&run(&mut p, "a=p,i=1,P=7,Q=1")).contains("ENOPARENT"));
    run(&mut p, "a=p,i=1,p=1");
    assert!(only_response(&run(&mut p, "a=p,i=1,p=2,P=1,Q=1")).contains("EINVAL"));
}

#[test]
fn placements_in_range_and_screens() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 60); // 3 rows
    let mut c = ctx();
    c.abs_row = 10;
    run_ctx(&mut p, "a=p,i=1,p=1", &c);
    c.abs_row = 50;
    run_ctx(&mut p, "a=p,i=1,p=2", &c);
    c.screen = Screen::Alt;
    c.abs_row = 11;
    run_ctx(&mut p, "a=p,i=1,p=3", &c);
    let ids = |s, r: std::ops::RangeInclusive<u64>| {
        let mut v: Vec<u32> = p.placements_in(s, r).map(|x| x.placement_id).collect();
        v.sort();
        v
    };
    assert_eq!(ids(Screen::Main, 0..=9), Vec::<u32>::new());
    assert_eq!(ids(Screen::Main, 0..=10), vec![1]);
    assert_eq!(ids(Screen::Main, 12..=12), vec![1], "tall image intersects via its lower rows");
    assert_eq!(ids(Screen::Main, 13..=49), Vec::<u32>::new());
    assert_eq!(ids(Screen::Main, 0..=100), vec![1, 2]);
    assert_eq!(ids(Screen::Alt, 0..=100), vec![3]);
    assert_eq!(p.placements_in(Screen::Main, 50..60).count(), 1);
    assert_eq!(p.placements_in(Screen::Main, 0..0).count(), 0);
}

// ------------------------------------------------------------------- delete

/// Three images with placements at known cells; returns a protocol.
fn scene() -> Protocol {
    let mut p = Protocol::new();
    setup(&mut p, 1, 20, 20); // 2x1 cells
    setup(&mut p, 2, 20, 20);
    setup(&mut p, 3, 20, 20);
    let mut c = ctx(); // abs_row 100, rows=24 => top 77 by fallback
    c.col = 0;
    c.abs_row = 90;
    run_ctx(&mut p, "a=p,i=1,p=1,z=0", &c); // cells x 0..1, row 90
    c.col = 10;
    c.abs_row = 91;
    run_ctx(&mut p, "a=p,i=2,p=1,z=-1", &c); // cols 10..11, row 91
    c.col = 10;
    c.abs_row = 91;
    run_ctx(&mut p, "a=p,i=2,p=2,z=3", &c); // same cell, other z
    c.col = 40;
    c.abs_row = 5; // scrolled off
    run_ctx(&mut p, "a=p,i=3,p=1", &c);
    p.set_viewport_top(Screen::Main, 80);
    p
}

fn left(p: &Protocol) -> Vec<(u32, u32)> {
    let mut v: Vec<_> = p.placements_in(Screen::Main, 0..=u64::MAX).map(|x| (x.image_id, x.placement_id)).collect();
    v.sort();
    v
}

#[test]
fn delete_all_visible_keeps_data_lowercase() {
    let mut p = scene();
    let a = run(&mut p, "a=d");
    assert!(has_dirty(&a));
    assert!(responses(&a).is_empty());
    assert_eq!(left(&p), vec![(3, 1)], "scrolled-off placement survives");
    assert_eq!(p.image_count(), 3, "lowercase keeps image data");
}

#[test]
fn delete_all_visible_uppercase_frees_data() {
    let mut p = scene();
    run(&mut p, "a=d,d=A");
    assert_eq!(left(&p), vec![(3, 1)]);
    assert!(p.image(1).is_none() && p.image(2).is_none());
    assert!(p.image(3).is_some(), "image 3 still has a placement");
}

#[test]
fn delete_by_id() {
    let mut p = scene();
    run(&mut p, "a=d,d=i,i=2,p=1");
    assert_eq!(left(&p), vec![(1, 1), (2, 2), (3, 1)]);
    run(&mut p, "a=d,d=i,i=2");
    assert_eq!(left(&p), vec![(1, 1), (3, 1)]);
    assert!(p.image(2).is_some());
    run(&mut p, "a=d,d=I,i=2");
    assert!(p.image(2).is_none());
    assert_eq!(p.take_removed_images(), vec![2]);
    // d=I on an image with no placements still frees it.
    run(&mut p, "a=d,d=I,i=1,p=1");
    assert!(p.image(1).is_none(), "uppercase frees data once the last placement is gone");
    // Placement-specific uppercase keeps data while other placements remain.
    let mut q = scene();
    run(&mut q, "a=d,d=I,i=2,p=1");
    assert!(q.image(2).is_some());
    assert_eq!(left(&q), vec![(1, 1), (2, 2), (3, 1)]);
    // d=I on an image with no placements still frees it.
    setup(&mut q, 50, 1, 1);
    run(&mut q, "a=d,d=I,i=50");
    assert!(q.image(50).is_none());
}

#[test]
fn delete_by_number_targets_newest() {
    let mut p = Protocol::new();
    let r1 = only_response(&run(&mut p, &format!("I=5,s=1,v=1;{}", rgba_b64(1, 1))));
    let r2 = only_response(&run(&mut p, &format!("I=5,s=1,v=1;{}", rgba_b64(1, 1))));
    let get = |r: &str| -> u32 { r["\x1b_Gi=".len()..].split(',').next().unwrap().parse().unwrap() };
    let (id1, id2) = (get(&r1), get(&r2));
    run(&mut p, &format!("a=p,i={id1},p=1"));
    run(&mut p, &format!("a=p,i={id2},p=1"));
    run(&mut p, "a=d,d=N,I=5");
    assert!(p.image(id2).is_none());
    assert!(p.image(id1).is_some());
    assert_eq!(p.placement_count(), 1);
}

#[test]
fn delete_at_cursor() {
    let mut p = scene();
    let mut c = ctx();
    c.col = 11;
    c.abs_row = 91;
    run_ctx(&mut p, "a=d,d=c", &c);
    assert_eq!(left(&p), vec![(1, 1), (3, 1)]);
    c.col = 12;
    c.abs_row = 90;
    let before = p.placement_count();
    run_ctx(&mut p, "a=d,d=c", &c);
    assert_eq!(p.placement_count(), before, "cursor outside any placement deletes nothing");
}

#[test]
fn delete_at_cell_uses_one_based_screen_coords() {
    let mut p = scene(); // top = 80, so row 90 is screen y=11, row 91 is y=12
    run(&mut p, "a=d,d=p,x=1,y=11");
    assert_eq!(left(&p), vec![(2, 1), (2, 2), (3, 1)]);
    run(&mut p, "a=d,d=p,x=12,y=12");
    assert_eq!(left(&p), vec![(3, 1)]);
    assert!(p.image(2).is_some());
    let mut p = scene();
    run(&mut p, "a=d,d=P,x=11,y=12");
    assert!(p.image(2).is_none());
}

#[test]
fn delete_at_cell_with_z() {
    let mut p = scene();
    run(&mut p, "a=d,d=q,x=11,y=12,z=3");
    assert_eq!(left(&p), vec![(1, 1), (2, 1), (3, 1)]);
    run(&mut p, "a=d,d=Q,x=11,y=12,z=-1");
    assert_eq!(left(&p), vec![(1, 1), (3, 1)]);
    assert!(p.image(2).is_none(), "uppercase freed the now-orphaned image");
}

#[test]
fn delete_by_id_range() {
    let mut p = scene();
    run(&mut p, "a=d,d=r,x=1,y=2");
    assert_eq!(left(&p), vec![(3, 1)]);
    assert_eq!(p.image_count(), 3);
    let mut p = scene();
    run(&mut p, "a=d,d=R,x=2,y=3");
    assert_eq!(p.image_count(), 1);
    assert_eq!(left(&p), vec![(1, 1)]);
}

#[test]
fn delete_by_column_and_row() {
    let mut p = scene();
    run(&mut p, "a=d,d=x,x=12"); // column index 11
    assert_eq!(left(&p), vec![(1, 1), (3, 1)]);
    let mut p = scene();
    run(&mut p, "a=d,d=y,y=11"); // row 90
    assert_eq!(left(&p), vec![(2, 1), (2, 2), (3, 1)]);
    let mut p = scene();
    run(&mut p, "a=d,d=X,x=1");
    assert!(p.image(1).is_none());
    let mut p = scene();
    run(&mut p, "a=d,d=Y,y=12");
    assert!(p.image(2).is_none());
}

#[test]
fn delete_by_z_index() {
    let mut p = scene();
    run(&mut p, "a=d,d=z,z=-1");
    assert_eq!(left(&p), vec![(1, 1), (2, 2), (3, 1)]);
    assert!(p.image(2).is_some());
    run(&mut p, "a=d,d=Z,z=3");
    assert!(p.image(2).is_none());
}

#[test]
fn delete_only_affects_current_screen_for_positional_keys() {
    let mut p = scene();
    let mut c = ctx();
    c.screen = Screen::Alt;
    run_ctx(&mut p, "a=d,d=a", &c);
    assert_eq!(p.placement_count(), 4);
    run_ctx(&mut p, "a=d,d=z,z=0", &c);
    assert_eq!(p.placement_count(), 4);
}

#[test]
fn delete_unknown_specifier_and_frames() {
    let mut p = scene();
    assert!(responses(&run(&mut p, "a=d,d=f")).is_empty());
    assert_eq!(p.placement_count(), 4);
    assert!(only_response(&run(&mut p, "a=d,d=!,i=1")).is_err_like());
}

trait ErrLike {
    fn is_err_like(&self) -> bool;
}
impl ErrLike for String {
    fn is_err_like(&self) -> bool {
        self.contains("EINVAL")
    }
}

#[test]
fn anonymous_images_are_freed_with_last_placement() {
    let mut p = Protocol::new();
    run(&mut p, &format!("a=T,s=1,v=1;{}", rgba_b64(1, 1)));
    assert_eq!(p.image_count(), 1);
    run(&mut p, "a=d");
    assert_eq!(p.image_count(), 0);
    assert_eq!(p.memory_used(), 0);
}

#[test]
fn gc_below_and_clear_screen() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 40); // 2 rows
    let mut c = ctx();
    for (i, row) in [10u64, 20, 30].iter().enumerate() {
        c.abs_row = *row;
        run_ctx(&mut p, &format!("a=p,i=1,p={}", i + 1), &c);
    }
    c.screen = Screen::Alt;
    run_ctx(&mut p, "a=p,i=1,p=9", &c);
    p.gc_below(Screen::Main, 21); // row 20 spans 20..=21, still alive
    let mut ids: Vec<u32> = p.placements_in(Screen::Main, 0..=u64::MAX).map(|x| x.placement_id).collect();
    ids.sort();
    assert_eq!(ids, vec![2, 3]);
    p.gc_below(Screen::Main, 22);
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).count(), 1);
    assert_eq!(p.placements_in(Screen::Alt, 0..=u64::MAX).count(), 1, "gc is per screen");
    p.clear_screen(Screen::Alt);
    assert_eq!(p.placements_in(Screen::Alt, 0..=u64::MAX).count(), 0);
    assert_eq!(p.placements_in(Screen::Main, 0..=u64::MAX).count(), 1, "main untouched");
    assert!(p.image(1).is_some(), "named images keep their data");
}

// ------------------------------------------------------------------- memory

#[test]
fn memory_cap_evicts_oldest_preferring_unplaced() {
    let mut p = Protocol::with_memory_limit(16 * 3); // three 2x2 images
    for id in 1..=3 {
        setup(&mut p, id, 2, 2);
    }
    assert_eq!(p.memory_used(), 48);
    // Oldest (1) has a placement; 2 is the oldest unplaced and goes first.
    run(&mut p, "a=p,i=1");
    setup(&mut p, 4, 2, 2);
    assert!(p.image(2).is_none());
    assert!(p.image(1).is_some() && p.image(3).is_some() && p.image(4).is_some());
    assert_eq!(p.memory_used(), 48);
    assert!(p.take_removed_images().contains(&2));
    // Now everything else is unplaced except 1: 3 is oldest.
    setup(&mut p, 5, 2, 2);
    assert!(p.image(3).is_none());
    // Evicting a placed image removes its placements and dirties.
    let mut q = Protocol::with_memory_limit(16);
    setup(&mut q, 1, 2, 2);
    run(&mut q, "a=p,i=1");
    let a = run(&mut q, &format!("i=2,s=2,v=2,f=32;{}", rgba_b64(2, 2)));
    assert!(has_dirty(&a));
    assert_eq!(q.placement_count(), 0);
    assert!(q.image(1).is_none() && q.image(2).is_some());
}

#[test]
fn image_bigger_than_quota_is_enospc_and_keeps_existing() {
    let mut p = Protocol::with_memory_limit(16);
    setup(&mut p, 1, 2, 2);
    let a = run(&mut p, &format!("i=2,s=3,v=3,f=32;{}", rgba_b64(3, 3)));
    assert!(only_response(&a).contains("ENOSPC"));
    assert!(p.image(1).is_some());
}

#[test]
fn replacing_same_id_does_not_double_count_memory() {
    let mut p = Protocol::with_memory_limit(16);
    setup(&mut p, 1, 2, 2);
    setup(&mut p, 1, 2, 2);
    assert_eq!(p.memory_used(), 16);
    assert!(p.image(1).is_some());
}

#[test]
fn lowering_limit_evicts() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 2, 2);
    setup(&mut p, 2, 2, 2);
    p.set_memory_limit(16);
    assert_eq!(p.image_count(), 1);
    assert!(p.image(2).is_some());
}

// -------------------------------------------------------------------- files

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn unique(tag: &str) -> String {
    format!("{}-{}-{}", tag, std::process::id(), COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
}

fn temp_file(name_tag: &str, contents: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(unique(name_tag));
    std::fs::write(&path, contents).unwrap();
    path
}

const EBADF: &str = "EBADF:Failed to read image file";

#[test]
fn file_medium_reads_regular_file_and_keeps_it() {
    let path = temp_file("grimalkin-plain", &[9, 8, 7, 255]);
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=1;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(&p.image(1).unwrap().rgba[..], &[9, 8, 7, 255]);
    assert!(path.exists(), "t=f must not delete the file");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn file_medium_offset_and_size() {
    let path = temp_file("grimalkin-off", &[0, 0, 0, 1, 2, 3, 4, 99, 99]);
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=1,O=3,S=4;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(&p.image(1).unwrap().rgba[..], &[1, 2, 3, 4]);
    // Smaller than claimed -> the same opaque error as any other read failure.
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=2,O=7,S=4;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert!(only_response(&a).ends_with(&format!("{EBADF}\x1b\\")));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn file_medium_png() {
    let path = temp_file("grimalkin-png", &png_bytes(3, 3));
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=f,f=100,i=1;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(p.image(1).unwrap().width, 3);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn file_failures_are_indistinguishable() {
    let mut p = Protocol::new();
    let dir = std::env::temp_dir().join(unique("grimalkin-dir"));
    std::fs::create_dir(&dir).unwrap();
    let missing = std::env::temp_dir().join(unique("grimalkin-missing"));
    let mut results = Vec::new();
    for path in [dir.to_str().unwrap().to_string(), missing.to_str().unwrap().to_string(), String::new()] {
        let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=1;{}", b64::encode(path.as_bytes())));
        results.push(only_response(&a));
    }
    assert!(results.iter().all(|r| r == &results[0]), "{results:?}");
    assert_eq!(results[0], format!("\x1b_Gi=1;{EBADF}\x1b\\"));
    // Invalid base64 for the path too.
    let a = run(&mut p, "t=f,f=32,s=1,v=1,i=1;!!!");
    assert_eq!(only_response(&a), results[0]);
    std::fs::remove_dir(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn file_medium_refuses_special_and_sensitive_files() {
    let mut p = Protocol::new();
    for path in ["/dev/null", "/dev/zero", "/proc/self/status", "/sys/kernel"] {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=1;{}", b64::encode(path.as_bytes())));
        assert_eq!(only_response(&a), format!("\x1b_Gi=1;{EBADF}\x1b\\"), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn file_medium_follows_symlinks_and_fails_on_loops() {
    let target = temp_file("grimalkin-target", &[1, 2, 3, 4]);
    let link = std::env::temp_dir().join(unique("grimalkin-link"));
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=1;{}", b64::encode(link.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), ok(1));
    let loop_a = std::env::temp_dir().join(unique("grimalkin-loopa"));
    let loop_b = std::env::temp_dir().join(unique("grimalkin-loopb"));
    std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
    std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=2;{}", b64::encode(loop_a.to_str().unwrap().as_bytes())));
    assert!(only_response(&a).contains("EBADF"));
    for f in [target, link, loop_a, loop_b] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn temp_file_medium_deletes_after_reading() {
    let path = temp_file("tty-graphics-protocol-ok", &[5, 6, 7, 8]);
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=t,f=32,s=1,v=1,i=1;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), ok(1));
    assert_eq!(&p.image(1).unwrap().rgba[..], &[5, 6, 7, 8]);
    assert!(!path.exists(), "temp file must be deleted");
}

#[test]
fn temp_file_without_marker_in_name_is_refused_and_kept() {
    let path = temp_file("grimalkin-no-marker", &[5, 6, 7, 8]);
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=t,f=32,s=1,v=1,i=1;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), format!("\x1b_Gi=1;{EBADF}\x1b\\"));
    assert!(path.exists());
    assert!(p.image(1).is_none());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn temp_file_outside_temp_dir_is_refused_and_kept() {
    // The marker is in the name but the directory is not a temp directory.
    let base = std::env::current_dir().unwrap().join("target");
    std::fs::create_dir_all(&base).unwrap();
    let path = base.join(unique("tty-graphics-protocol-outside"));
    std::fs::write(&path, [1, 2, 3, 4]).unwrap();
    // Skip if the build dir itself lives under the temp dir.
    let canon = std::fs::canonicalize(&path).unwrap();
    if canon.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()) {
        std::fs::remove_file(path).unwrap();
        return;
    }
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=t,f=32,s=1,v=1,i=1;{}", b64::encode(path.to_str().unwrap().as_bytes())));
    assert_eq!(only_response(&a), format!("\x1b_Gi=1;{EBADF}\x1b\\"));
    assert!(path.exists());
    std::fs::remove_file(path).unwrap();
    let _ = std::fs::remove_dir(&base);
}

#[test]
fn shared_memory_is_rejected() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=s,f=32,s=1,v=1,i=1;{}", b64::encode(b"/some-shm")));
    let r = only_response(&a);
    assert!(r.contains("EINVAL"), "{r}");
    assert_eq!(p.image_count(), 0);
}

#[test]
fn unknown_medium_is_rejected() {
    let mut p = Protocol::new();
    assert!(only_response(&run(&mut p, "t=x,f=32,s=1,v=1,i=1;AAAA")).contains("EINVAL"));
}

// --------------------------------------------------------------------- fuzz

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn fuzz_random_payloads_never_panic() {
    let mut rng = Lcg(0xC0FFEE);
    let mut p = Protocol::with_memory_limit(1 << 20);
    let keys = b"aqftsvSOiIpomxywhXYcrCUzdePQHVN";
    let vals: [&str; 14] = ["0", "1", "2", "3", "24", "32", "100", "-1", "99999999999", "t", "T", "z", "A", "x"];
    for iter in 0..10_000u32 {
        let mut buf: Vec<u8> = Vec::new();
        match rng.next() % 3 {
            0 => {
                // Pure noise.
                let n = (rng.next() % 200) as usize;
                for _ in 0..n {
                    buf.push(rng.next() as u8);
                }
            }
            1 => {
                // Plausible control data with random keys/values and junk payload.
                let n = rng.next() % 8;
                for k in 0..n {
                    if k > 0 {
                        buf.push(b',');
                    }
                    buf.push(keys[(rng.next() as usize) % keys.len()]);
                    buf.push(b'=');
                    buf.extend_from_slice(vals[(rng.next() as usize) % vals.len()].as_bytes());
                }
                buf.push(b';');
                let n = rng.next() % 64;
                for _ in 0..n {
                    buf.push(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/="[(rng.next() % 65) as usize]);
                }
            }
            _ => {
                // Valid-ish commands with a small real image to exercise stores/places/deletes.
                let id = rng.next() % 5;
                let cmd = match rng.next() % 6 {
                    0 => format!("a=T,i={id},f=32,s=2,v=2,c={},r={};{}", rng.next() % 5, rng.next() % 5, rgba_b64(2, 2)),
                    1 => format!("a=p,i={id},p={},x={},y={},w={},h={},z={}", rng.next() % 3, rng.next() % 4, rng.next() % 4, rng.next() % 4, rng.next() % 4, rng.next() % 3),
                    2 => format!("a=d,d={},i={id},x={},y={},z={}", ["a", "A", "i", "I", "c", "p", "q", "r", "x", "y", "z", "N", "Z"][(rng.next() % 13) as usize], rng.next() % 90, rng.next() % 30, rng.next() % 3),
                    3 => format!("i={id},f=100;{}", b64::encode(&png_bytes(2, 2))),
                    4 => format!("i={id},f=32,s=2,v=2,m=1;{}", &rgba_b64(2, 2)[..8]),
                    _ => format!("m={},q={};{}", rng.next() % 2, rng.next() % 3, &rgba_b64(2, 2)[8..]),
                };
                buf = cmd.into_bytes();
            }
        }
        let c = CursorCtx {
            col: (rng.next() % 100) as u16,
            abs_row: rng.next() % 1000,
            cols: (rng.next() % 100) as u16,
            rows: (rng.next() % 40) as u16,
            cell_w_px: (rng.next() % 3) as u32 * 5,
            cell_h_px: (rng.next() % 3) as u32 * 9,
            screen: if iter % 2 == 0 { Screen::Main } else { Screen::Alt },
        };
        let _ = p.handle(&buf, &c);
        let _ = p.placements_in(Screen::Main, 0..=1000).count();
        if iter % 97 == 0 {
            p.gc_below(Screen::Main, 500);
            p.clear_screen(Screen::Alt);
            p.take_dirty_images();
            p.take_removed_images();
        }
        assert!(p.memory_used() <= p.memory_limit());
    }
}

#[test]
fn extreme_context_values_do_not_overflow() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 10, 20);
    let c = CursorCtx { col: u16::MAX, abs_row: u64::MAX - 1, cols: 0, rows: 0, cell_w_px: 1, cell_h_px: 1, screen: Screen::Main };
    for cmd in ["a=p,i=1,c=65535,r=65535", "a=p,i=1,c=4294967295", "a=d,d=a", "a=d,d=y,y=4294967295", "a=d,d=c"] {
        let _ = run_ctx(&mut p, cmd, &c);
    }
    let _ = p.placements_in(Screen::Main, u64::MAX - 1..=u64::MAX).count();
    p.gc_below(Screen::Main, u64::MAX);
}

// ------------------------------------------------------- review regressions

#[test]
fn unlimited_memory_limit_does_not_overflow() {
    let mut p = Protocol::with_memory_limit(usize::MAX);
    let full = rgba_b64(1, 2);
    assert!(run(&mut p, &format!("f=32,s=1,v=2,i=1,m=1;{}", &full[..4])).is_empty());
    let a = run(&mut p, &format!("m=0;{}", &full[4..]));
    assert_eq!(only_response(&a), ok(1));
    // File reads use `limit + 1` internally.
    let path = std::env::temp_dir().join("grimalkin-gfx-unlimited.rgba");
    std::fs::write(&path, [1u8, 2, 3, 4]).unwrap();
    let enc = b64::encode(path.to_string_lossy().as_bytes());
    let a = run(&mut p, &format!("t=f,f=32,s=1,v=1,i=2;{enc}"));
    assert_eq!(only_response(&a), ok(2));
    let _ = std::fs::remove_file(path);
}

#[test]
fn reupload_after_removal_is_not_reported_removed() {
    let mut p = Protocol::new();
    run(&mut p, &format!("s=1,v=1,i=4;{}", rgba_b64(1, 1)));
    run(&mut p, "a=d,d=I,i=4");
    run(&mut p, &format!("s=1,v=1,i=4;{}", rgba_b64(1, 1)));
    assert!(p.take_removed_images().is_empty(), "image 4 exists again; its texture must be kept");
    assert_eq!(p.take_dirty_images(), vec![4]);
    // Replacing in place is likewise not a removal.
    run(&mut p, &format!("s=1,v=1,i=4;{}", rgba_b64(1, 1)));
    assert!(p.take_removed_images().is_empty());
}

#[test]
fn continuation_chunk_may_repeat_its_own_id() {
    let mut p = Protocol::new();
    let full = rgba_b64(1, 2);
    run(&mut p, &format!("f=32,s=1,v=2,i=8,m=1;{}", &full[..4]));
    let a = run(&mut p, &format!("i=8,m=0;{}", &full[4..]));
    assert_eq!(only_response(&a), ok(8));
    assert_eq!(b64::encode(&p.image(8).unwrap().rgba), full);
}

#[test]
fn new_transmission_without_action_key_aborts_partial_upload() {
    let mut p = Protocol::new();
    let full = rgba_b64(1, 1);
    run(&mut p, &format!("f=32,s=1,v=1,i=8,m=1;{}", &full[..4]));
    // No `a=` but it carries f/s/v/i: a brand new image, not a continuation.
    let a = run(&mut p, &format!("f=32,s=1,v=1,i=9;{full}"));
    assert_eq!(only_response(&a), ok(9));
    assert!(p.image(8).is_none());
    assert_eq!(p.image(9).unwrap().rgba.len(), 4);
    // A different id with only chunk keys is also not a continuation.
    run(&mut p, &format!("f=32,s=1,v=1,i=10,m=1;{}", &full[..4]));
    run(&mut p, &format!("i=11,m=0;{}", &full[4..]));
    assert!(p.image(10).is_none());
}

#[test]
fn draw_order_is_z_then_image_id() {
    let mut p = Protocol::new();
    for id in [3u32, 1, 2] {
        run(&mut p, &format!("s=1,v=1,i={id};{}", rgba_b64(1, 1)));
    }
    run(&mut p, "a=p,i=3");
    run(&mut p, "a=p,i=1,z=5");
    run(&mut p, "a=p,i=2,z=-1");
    run(&mut p, "a=p,i=1");
    let order: Vec<(i32, u32)> =
        p.placements_in_draw_order(Screen::Main, 0..).iter().map(|x| (x.z, x.image_id)).collect();
    assert_eq!(order, vec![(-1, 2), (0, 1), (0, 3), (5, 1)]);
}

#[test]
fn failed_display_of_anonymous_image_does_not_leak_it() {
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("a=T,s=1,v=1,x=5;{}", rgba_b64(1, 1)));
    assert!(responses(&a).is_empty());
    assert_eq!(p.image_count(), 0);
    assert_eq!(p.memory_used(), 0);
}

// ------------------------------------------------- adversarial-QA regressions

/// F4: `f=100` ignores `s`/`v`, so absurd values must not reach any
/// width * height arithmetic (debug builds panicked on the multiplication).
#[test]
fn huge_dimensions_with_png_format_do_not_overflow() {
    let mut p = Protocol::new();
    for keys in ["f=100", "f=100,o=z", "f=100,t=f", "f=24", "f=32", "f=32,o=z", "f=24,o=z"] {
        let a = run(&mut p, &format!("a=q,i=1,{keys},s=4294967295,v=4294967295;AAAA"));
        let r = only_response(&a);
        assert!(r.starts_with("\x1b_Gi=1;E"), "{keys}: {r:?}");
    }
    // A PNG whose own header is the huge part is refused too.
    let mut png = png_bytes(2, 2);
    png[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    png[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    let a = run(&mut p, &format!("a=q,i=1,f=100;{}", b64::encode(&png)));
    assert!(only_response(&a).starts_with("\x1b_Gi=1;E"));
    assert_eq!(p.image_count(), 0);
}

/// F20: whatever is wrong with a file, the sender sees the same error, so it
/// cannot probe for local files or learn their size or type.
#[test]
fn file_media_errors_are_uniform() {
    let big = temp_file("grimalkin-probe-big", &[7u8; 9350]);
    let short = temp_file("grimalkin-probe-short", &[7u8; 3]);
    let junk_z = temp_file("grimalkin-probe-z", b"not zlib at all, but long enough");
    let junk_png = temp_file("grimalkin-probe-png", b"definitely not a png file");
    let empty = temp_file("grimalkin-probe-empty", b"");
    let missing = std::env::temp_dir().join(unique("grimalkin-probe-missing"));
    let enc = |p: &std::path::Path| b64::encode(p.to_str().unwrap().as_bytes());
    for (keys, path) in [
        ("f=32,s=1,v=1", &big),
        ("f=32,s=1,v=1", &short),
        ("f=24,s=1,v=1", &big),
        ("f=32,s=1,v=1,o=z", &junk_z),
        ("f=32,s=1,v=1,o=z", &big),
        ("f=100", &junk_png),
        ("f=100,o=z", &junk_z),
        ("f=32,s=1,v=1", &empty),
        ("f=32,s=0,v=0", &big),
        ("f=32,s=99999,v=99999", &big),
        ("f=32,s=1,v=1", &missing),
    ] {
        let mut p = Protocol::new();
        let a = run(&mut p, &format!("t=f,i=1,{keys};{}", enc(path)));
        assert_eq!(only_response(&a), format!("\x1b_Gi=1;{EBADF}\x1b\\"), "{keys}");
        assert_eq!(p.image_count(), 0);
    }
    // A temp file read as t=t is just as uniform (and still deleted).
    let tmp = std::env::temp_dir().join(format!("tty-graphics-protocol-{}", unique("probe")));
    std::fs::write(&tmp, [1u8; 100]).unwrap();
    let mut p = Protocol::new();
    let a = run(&mut p, &format!("t=t,i=1,f=32,s=1,v=1;{}", enc(&tmp)));
    assert_eq!(only_response(&a), format!("\x1b_Gi=1;{EBADF}\x1b\\"));
    assert!(!tmp.exists());
    // Direct transmission keeps its descriptive errors.
    let a = run(&mut p, &format!("i=2,f=32,s=1,v=1;{}", b64::encode(&[0u8; 9])));
    assert!(only_response(&a).contains("EFBIG:Too much image data: 9 > 4"));
    for f in [big, short, junk_z, junk_png, empty] {
        std::fs::remove_file(f).unwrap();
    }
}

/// F9: re-placing with ever new placement ids was quadratic, and nothing
/// bounded the number of placements.
#[test]
fn placement_floods_are_fast_and_capped() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 1, 1);
    let start = std::time::Instant::now();
    let n = 3 * MAX_PLACEMENTS;
    for k in 1..=n {
        run(&mut p, &format!("a=p,i=1,p={k},q=2"));
    }
    assert!(p.placement_count() <= MAX_PLACEMENTS);
    assert!(p.placement_count() > MAX_PLACEMENTS / 2);
    // The oldest are the ones dropped; the newest survive.
    assert!(p.placements_in(Screen::Main, 0..).any(|x| x.placement_id == n as u32));
    assert!(!p.placements_in(Screen::Main, 0..).any(|x| x.placement_id == 1));
    // Id-less placements and virtual ones are capped as well.
    for _ in 0..2 * MAX_PLACEMENTS {
        run(&mut p, "a=p,i=1,q=2");
        run(&mut p, "a=p,i=1,U=1,q=2");
    }
    assert!(p.placement_count() <= MAX_PLACEMENTS);
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "{:?}", start.elapsed());
}

#[test]
fn placement_replacement_by_id_keeps_one_and_indexes_stay_consistent() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 1, 1);
    setup(&mut p, 2, 1, 1);
    for _ in 0..5 {
        run(&mut p, "a=p,i=1,p=7,q=2");
        run(&mut p, "a=p,i=2,p=7,q=2");
    }
    assert_eq!(p.placement_count(), 2);
    run(&mut p, "a=d,d=i,i=1,p=7");
    assert_eq!(p.placement_count(), 1);
    run(&mut p, "a=p,i=1,p=7,q=2");
    assert_eq!(p.placement_count(), 2);
    run(&mut p, "a=d,d=I,i=1");
    assert_eq!(p.placement_count(), 1);
    assert!(p.image(1).is_none());
    run(&mut p, "a=d,d=A");
    assert_eq!(p.placement_count(), 0);
    assert_eq!(p.image_count(), 0);
    assert!(p.by_pid.is_empty() && p.by_image.is_empty() && p.lru.is_empty());
}

/// F10: evicting N small images to make room for one big one must not cost
/// O(N) per victim.
#[test]
fn evicting_many_small_images_is_linear() {
    let n = MAX_IMAGES as u32;
    let mut p = Protocol::with_memory_limit(8 * 1024 * 1024);
    let start = std::time::Instant::now();
    for i in 1..=n {
        run(&mut p, &format!("a=t,f=24,s=1,v=1,i={i},q=2;/wAA"));
    }
    assert_eq!(p.image_count(), n as usize);
    let _ = p.take_dirty_images();
    // One image that needs nearly the whole quota.
    let big = b64::encode(&zlib(&vec![0u8; 1024 * 2048 * 4]));
    let a = run(&mut p, &format!("a=t,f=32,s=1024,v=2048,i={},o=z,q=2;{big}", n + 1));
    assert!(responses(&a).is_empty(), "{a:?}");
    assert!(p.image(n + 1).is_some());
    assert!(p.memory_used() <= p.memory_limit());
    assert_eq!(p.image_count(), 1);
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "{:?}", start.elapsed());
    // Every evicted image is reported removed exactly once.
    let removed = p.take_removed_images();
    assert!(removed.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(removed.len() + p.image_count() - 1, n as usize);
    assert_eq!(p.take_dirty_images(), vec![n + 1]);
}

#[test]
fn eviction_prefers_unplaced_then_least_recently_used() {
    // Like `setup`, but eviction may legitimately report a redraw.
    fn put(p: &mut Protocol, id: u32, w: u32, h: u32) {
        run(p, &format!("i={id},s={w},v={h},f=32,q=2;{}", rgba_b64(w, h)));
    }
    let mut p = Protocol::with_memory_limit(16 * 3);
    for id in 1..=3u32 {
        put(&mut p, id, 2, 2);
    }
    run(&mut p, "a=p,i=1,q=2"); // placed and oldest
    run(&mut p, "a=p,i=3,q=2");
    run(&mut p, "a=p,i=2,U=1,q=2"); // virtual placements count as placed too
    put(&mut p, 4, 2, 2);
    // All three had placements; the least recently used (1) goes.
    assert!(p.image(1).is_none() && p.image(2).is_some() && p.image(3).is_some());
    put(&mut p, 5, 2, 2);
    // 4 has no placement, so it is the first victim even though it is newer.
    assert!(p.image(4).is_none() && p.image(2).is_some() && p.image(3).is_some() && p.image(5).is_some());
    run(&mut p, "a=p,i=5,q=2");
    put(&mut p, 6, 2, 2);
    // Everything is placed again: 3 was put least recently.
    assert!(p.image(3).is_none() && p.image(2).is_some() && p.image(5).is_some() && p.image(6).is_some());
}

/// F26: the number of images is bounded too, not only their pixel bytes.
#[test]
fn image_count_is_capped() {
    let mut p = Protocol::new();
    for i in 1..=(MAX_IMAGES as u32 + 100) {
        run(&mut p, &format!("a=t,f=24,s=1,v=1,i={i},q=2;/wAA"));
    }
    assert_eq!(p.image_count(), MAX_IMAGES);
    assert!(p.image(1).is_none());
    assert!(p.image(MAX_IMAGES as u32 + 100).is_some());
}

/// F26: delete-all (and so ED 2 / RIS, which send it) clears virtual
/// placements, which have no position for the position-based deletes.
#[test]
fn delete_all_clears_virtual_placements() {
    let mut p = Protocol::new();
    setup(&mut p, 1, 1, 1);
    for _ in 0..1000 {
        run(&mut p, "a=p,i=1,U=1,q=2");
    }
    assert_eq!(p.virtual_placements(Screen::Main).count(), 1000);
    run(&mut p, "a=d,d=A");
    assert_eq!(p.virtual_placements(Screen::Main).count(), 0);
    assert_eq!(p.placement_count(), 0);
}

/// F32: pixels land in the right place for every decode path, including
/// ones that stream in chunks.
#[test]
fn streamed_decoding_is_exact_for_every_format() {
    let (w, h) = (300u32, 100u32); // 30000 px: more than one 16K-pixel chunk
    let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 253) as u8).collect();
    let want: Vec<u8> = rgb.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    let mut p = Protocol::new();
    for (i, keys, data) in [
        (1, "f=24", rgb.clone()),
        (2, "f=24,o=z", zlib(&rgb)),
        (3, "f=32,o=z", zlib(&want)),
        (4, "f=32", want.clone()),
    ] {
        let a = run(&mut p, &format!("i={i},s={w},v={h},{keys};{}", b64::encode(&data)));
        assert_eq!(only_response(&a), ok(i));
        assert_eq!(&p.image(i).unwrap().rgba[..], &want[..], "{keys}");
    }
    // Truncated and over-long streams.
    let a = run(&mut p, &format!("i=5,s={w},v={h},f=24,o=z;{}", b64::encode(&zlib(&rgb[..rgb.len() - 1]))));
    assert!(only_response(&a).contains("ENODATA:Insufficient image data"));
    let mut long = rgb.clone();
    long.push(0);
    let a = run(&mut p, &format!("i=5,s={w},v={h},f=24,o=z;{}", b64::encode(&zlib(&long))));
    assert!(only_response(&a).contains("EFBIG:Too much image data"));
    let a = run(&mut p, &format!("i=5,s={w},v={h},f=24,o=z;{}", b64::encode(&[0x78, 0x9c, 1, 2, 3, 4])));
    assert!(only_response(&a).contains("EINVAL:Failed to inflate"));
    assert!(p.image(5).is_none());
}

#[test]
fn png_color_types_decode_to_rgba() {
    let mut p = Protocol::new();
    let enc = |img: image::DynamicImage| {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        b64::encode(&out)
    };
    let luma = image::GrayImage::from_raw(2, 1, vec![10, 200]).unwrap();
    let la = image::GrayAlphaImage::from_raw(2, 1, vec![10, 20, 200, 30]).unwrap();
    let rgb = image::RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
    let rgb16 = image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(1, 1, vec![0xffffu16, 0, 0x8080]).unwrap();
    for (i, img, want) in [
        (1u32, image::DynamicImage::ImageLuma8(luma), vec![10, 10, 10, 255, 200, 200, 200, 255]),
        (2, image::DynamicImage::ImageLumaA8(la), vec![10, 10, 10, 20, 200, 200, 200, 30]),
        (3, image::DynamicImage::ImageRgb8(rgb), vec![1, 2, 3, 255, 4, 5, 6, 255]),
        (4, image::DynamicImage::ImageRgb16(rgb16), vec![255, 0, 128, 255]),
    ] {
        let a = run(&mut p, &format!("i={i},f=100;{}", enc(img)));
        assert_eq!(only_response(&a), ok(i));
        assert_eq!(&p.image(i).unwrap().rgba[..], &want[..], "image {i}");
    }
    let a = run(&mut p, &format!("i=9,f=100;{}", b64::encode(&png_bytes(3, 2))));
    assert_eq!(only_response(&a), ok(9));
    assert_eq!(p.image(9).unwrap().rgba.len(), 24);
}

/// F32: room is made before the new pixels are allocated (so old and new are
/// not resident together), but only for transmissions known to be good.
#[test]
fn eviction_happens_only_for_valid_uploads() {
    let mut p = Protocol::with_memory_limit(16 * 2);
    for id in 1..=2u32 {
        run(&mut p, &format!("i={id},s=2,v=2,f=32,q=2;{}", rgba_b64(2, 2)));
    }
    assert_eq!(p.image_count(), 2);
    let px = vec![9u8; 16];
    let bad = [
        // truncated, over-long, bad stream, and a query that would fit
        format!("i=3,s=2,v=2,f=32,o=z;{}", b64::encode(&zlib(&px[..15]))),
        format!("i=3,s=2,v=2,f=32,o=z;{}", b64::encode(&zlib(&[px.clone(), vec![0]].concat()))),
        format!("i=3,s=2,v=2,f=32,o=z;{}", b64::encode(&[0x78, 0x9c, 1, 2, 3])),
        format!("i=3,s=2,v=2,f=32;{}", b64::encode(&px[..15])),
        format!("a=q,i=3,s=2,v=2,f=32;{}", b64::encode(&px)),
    ];
    for cmd in &bad {
        let a = run(&mut p, cmd);
        assert_eq!(p.image_count(), 2, "{cmd}");
        assert!(p.take_removed_images().is_empty(), "{cmd}: {a:?}");
    }
    // A valid one evicts exactly what it needs, compressed or not.
    for (id, cmd) in [
        (3, format!("i=3,s=2,v=2,f=32,o=z;{}", b64::encode(&zlib(&px)))),
        (4, format!("i=4,s=2,v=2,f=32;{}", b64::encode(&px))),
    ] {
        let a = run(&mut p, &cmd);
        assert_eq!(only_response(&a), ok(id));
        assert_eq!(&p.image(id).unwrap().rgba[..], &px[..]);
        assert_eq!(p.image_count(), 2);
    }
    // Re-sending an id at full quota replaces it instead of evicting others.
    let mut q = Protocol::with_memory_limit(16 * 2);
    for id in 1..=2u32 {
        run(&mut q, &format!("i={id},s=2,v=2,f=32,q=2;{}", rgba_b64(2, 2)));
    }
    let a = run(&mut q, &format!("i=2,s=2,v=2,f=32,o=z;{}", b64::encode(&zlib(&px))));
    assert_eq!(only_response(&a), ok(2));
    assert!(q.image(1).is_some());
    assert_eq!(&q.image(2).unwrap().rgba[..], &px[..]);
}
