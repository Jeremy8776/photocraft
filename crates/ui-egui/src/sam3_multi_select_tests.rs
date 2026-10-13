use super::*;
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};

fn png(pixels: Vec<u8>, width: u32) -> Vec<u8> {
    let height = pixels.len() as u32 / width;
    let image = Image::from_u8(width, height, ChannelLayout::Gray, pixels).unwrap();
    photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap()
}

#[test]
fn unmatched_phrases_do_not_discard_successful_matches() {
    let mut seen = Vec::new();
    let (words, mask) = ground_phrases("cat, bird, dog", |phrase| {
        seen.push(phrase.to_owned());
        match phrase {
            "cat" => Ok(png(vec![255, 0], 2)),
            "dog" => Ok(png(vec![0, 255], 2)),
            _ => Err("Nothing matched “bird”".into()),
        }
    })
    .unwrap();
    assert_eq!(seen, ["cat", "bird", "dog"]);
    assert_eq!(words, "cat, dog");
    assert_eq!(photocraft_codecs::decode_as(Format::Png, &mask).unwrap().as_u8().unwrap(), &[255, 255]);
}

#[test]
fn whitespace_empty_entries_and_duplicates_do_not_create_extra_requests() {
    let mut seen = Vec::new();
    let (words, _) = ground_phrases(" , cat, cat, , red  ball, dog, ", |phrase| {
        seen.push(phrase.to_owned());
        Ok(png(vec![255], 1))
    })
    .unwrap();
    assert_eq!(seen, ["cat", "red  ball", "dog"]);
    assert_eq!(words, "cat, red  ball, dog");
}

#[test]
fn all_unmatched_phrases_report_no_selection() {
    let mut seen = Vec::new();
    let error = ground_phrases("cat, dog", |phrase| {
        seen.push(phrase.to_owned());
        Err(format!("Nothing matched “{phrase}”"))
    })
    .unwrap_err();
    assert_eq!(seen, ["cat", "dog"]);
    assert_eq!(error, "Nothing matched “cat, dog”");
}

#[test]
fn provider_failures_abort_instead_of_applying_incomplete_results() {
    let mut seen = Vec::new();
    let result = ground_phrases("cat, dog, ball", |phrase| {
        seen.push(phrase.to_owned());
        if phrase == "cat" { Ok(png(vec![255], 1)) } else { Err("connection lost".into()) }
    });
    assert_eq!(result.unwrap_err(), "connection lost");
    assert_eq!(seen, ["cat", "dog"]);
}

#[test]
fn invalid_phrase_lists_are_rejected_before_contacting_the_model() {
    for input in [" , , ".to_owned(), "a".repeat(201), (0..33).map(|n| n.to_string()).collect::<Vec<_>>().join(","), "a".repeat(7000)] {
        let mut calls = 0;
        assert!(
            ground_phrases(&input, |_| {
                calls += 1;
                Ok(png(vec![255], 1))
            })
            .is_err()
        );
        assert_eq!(calls, 0);
    }
}

#[test]
fn mismatched_or_invalid_masks_are_rejected() {
    let mut calls = 0;
    assert!(
        ground_phrases("cat,dog", |_| {
            calls += 1;
            Ok(if calls == 1 { png(vec![255], 1) } else { png(vec![255, 255], 2) })
        })
        .unwrap_err()
        .contains("dimensions")
    );
    assert!(ground_phrases("cat", |_| Ok(b"not a PNG".to_vec())).is_err());
    let rgb = Image::from_u8(1, 1, ChannelLayout::Rgb, vec![255; 3]).unwrap();
    let rgb_png = photocraft_codecs::encode(&rgb, Format::Png, &EncodeOptions::default()).unwrap();
    assert!(ground_phrases("cat", |_| Ok(rgb_png.clone())).unwrap_err().contains("grayscale"));
}

#[test]
fn union_preserves_partial_mask_coverage() {
    let (_, mask) = ground_phrases("cat, dog", |phrase| Ok(png(if phrase == "cat" { vec![90, 180] } else { vec![140, 120] }, 2))).unwrap();
    assert_eq!(photocraft_codecs::decode_as(Format::Png, &mask).unwrap().as_u8().unwrap(), &[140, 180]);
}

#[test]
fn phrase_union_is_applied_once_in_every_selection_mode() {
    use base64::Engine;
    use serde_json::json;
    let (_, mask) = ground_phrases("cat, ball", |phrase| Ok(png(if phrase == "cat" { vec![255, 0, 0] } else { vec![0, 0, 255] }, 3))).unwrap();
    for depth in [8, 16, 32] {
        for (mode, expected) in [("replace", [1.0, 0.0, 1.0]), ("add", [1.0, 1.0, 1.0]), ("subtract", [0.0, 1.0, 0.0]), ("intersect", [1.0, 0.0, 0.0])] {
            let mut session = photocraft_engine::Session::new();
            session.execute("file.new", json!({"width":3,"height":1,"depth":depth})).unwrap();
            session.execute("select.rect", json!({"x":0,"y":0,"width":2,"height":1})).unwrap();
            let st = session.active().unwrap();
            let before = st.history.past_len();
            let params = json!({"docId":st.doc.id.0,"revision":st.revision,"mode":mode,"maskPng":base64::engine::general_purpose::STANDARD.encode(&mask)});
            session.execute("select.sam3Mask", params).unwrap();
            let st = session.active().unwrap();
            assert_eq!(st.history.past_len(), before + 1, "one undo step per phrase list");
            let selected = st.doc.selection.as_ref().unwrap();
            assert_eq!([selected.pixel(0, 0)[0], selected.pixel(1, 0)[0], selected.pixel(2, 0)[0]], expected, "{mode}, depth {depth}");
        }
    }
}

#[test]
fn commas_separate_phrases_and_matches_are_unioned() {
    let mut seen = Vec::new();
    let (words, mask) = ground_phrases(" Cat, Dog, red ball ", |phrase| {
        seen.push(phrase.to_owned());
        Ok(png(
            match phrase {
                "Cat" => vec![255, 0, 90],
                "Dog" => vec![0, 255, 180],
                "red ball" => vec![0, 0, 255],
                _ => vec![0; 3],
            },
            3,
        ))
    })
    .unwrap();
    assert_eq!(seen, ["Cat", "Dog", "red ball"]);
    assert_eq!(words, "Cat, Dog, red ball");
    let image = photocraft_codecs::decode_as(Format::Png, &mask).unwrap();
    assert_eq!(image.as_u8().unwrap(), &[255, 255, 255]);
}
