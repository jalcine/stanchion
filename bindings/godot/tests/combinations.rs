//! Loads the chess-combination plugins that ship with the `examples/chess` game and
//! exercises each one's `suggest` logic through the real stanchion runtime.
//!
//! This is the Godot-free half of the example's test story: it proves the Lua plugins
//! load in the sandbox and pick the move their combination is meant to pick, given a
//! position shaped exactly as `ChessBoard.position_for` shapes it in GDScript. The
//! GDScript rules engine itself is covered by playing the game in the editor.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use stanchion_ffi::{HostConfig, Stanchion, Value};

/// The `examples/chess/plugins` directory, relative to this crate.
fn plugins_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/chess/plugins")
}

fn host() -> Stanchion {
    let config = HostConfig {
        plugins: Some(plugins_dir()),
        ..HostConfig::default()
    };
    let host = Stanchion::builder()
        .config(config)
        .build()
        .expect("host builds");
    let report = host.load(None).expect("plugins load");
    assert!(
        report.is_clean(),
        "every combination plugin should load: {:?}",
        report.failures
    );
    host
}

// ---- helpers to build a position the way GDScript does ---------------------

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn map(pairs: &[(&str, Value)]) -> Value {
    Value::Map(pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect())
}

/// One annotated move, with the fields `annotated_moves` fills and sane defaults.
#[derive(Clone)]
struct Move {
    from: i64,
    to: i64,
    piece: &'static str,
    to_sq: &'static str,
    from_sq: &'static str,
    captured_value: i64,
    capture: &'static str,
    is_castle: bool,
    is_mate: bool,
    promotes: &'static str,
    attacks_valuable: i64,
    attacks: Vec<&'static str>,
}

impl Default for Move {
    fn default() -> Self {
        Move {
            from: 0,
            to: 0,
            piece: "P",
            to_sq: "e4",
            from_sq: "e2",
            captured_value: 0,
            capture: "",
            is_castle: false,
            is_mate: false,
            promotes: "",
            attacks_valuable: 0,
            attacks: Vec::new(),
        }
    }
}

impl Move {
    fn value(&self) -> Value {
        let mut entries = BTreeMap::new();
        entries.insert("from".into(), Value::Int(self.from));
        entries.insert("to".into(), Value::Int(self.to));
        entries.insert("piece".into(), s(self.piece));
        entries.insert("to_sq".into(), s(self.to_sq));
        entries.insert("from_sq".into(), s(self.from_sq));
        entries.insert("captured_value".into(), Value::Int(self.captured_value));
        entries.insert("capture".into(), s(self.capture));
        entries.insert("is_castle".into(), Value::Bool(self.is_castle));
        entries.insert("is_en_passant".into(), Value::Bool(false));
        entries.insert("is_mate".into(), Value::Bool(self.is_mate));
        entries.insert("gives_check".into(), Value::Bool(self.is_mate));
        entries.insert("promotes".into(), s(self.promotes));
        entries.insert("attacks_valuable".into(), Value::Int(self.attacks_valuable));
        entries.insert(
            "attacks".into(),
            Value::List(self.attacks.iter().map(|a| s(a)).collect()),
        );
        Value::Map(entries)
    }
}

fn position(fullmove: i64, moves: &[Move]) -> Value {
    map(&[
        ("side", s("w")),
        ("fullmove", Value::Int(fullmove)),
        ("in_check", Value::Bool(false)),
        ("moves", Value::List(moves.iter().map(Move::value).collect())),
    ])
}

/// Reads `from`/`to` out of a suggestion, or `None` when the plugin declined.
fn picked(value: &Value) -> Option<(i64, i64)> {
    let Value::Map(entries) = value else {
        return None;
    };
    let from = match entries.get("from") {
        Some(Value::Int(from)) => *from,
        _ => return None,
    };
    let to = match entries.get("to") {
        Some(Value::Int(to)) => *to,
        _ => return None,
    };
    Some((from, to))
}

// ---- one test per combination ----------------------------------------------

#[test]
fn checkmate_prefers_the_mating_move() {
    let host = host();
    let moves = [
        Move { from: 10, to: 26, ..Default::default() },
        Move { from: 3, to: 39, piece: "Q", to_sq: "h5", is_mate: true, ..Default::default() },
    ];
    let picked = picked(&host.call("checkmate", "suggest", &[position(10, &moves)]).unwrap());
    assert_eq!(picked, Some((3, 39)), "should play the mate");
}

#[test]
fn win_material_takes_the_biggest_piece() {
    let host = host();
    let moves = [
        Move { from: 12, to: 20, ..Default::default() },
        Move { from: 12, to: 28, capture: "P", captured_value: 1, to_sq: "e5", ..Default::default() },
        Move { from: 33, to: 54, piece: "B", capture: "Q", captured_value: 9, to_sq: "g7", ..Default::default() },
    ];
    let picked = picked(&host.call("win_material", "suggest", &[position(10, &moves)]).unwrap());
    assert_eq!(picked, Some((33, 54)), "should grab the queen, not the pawn");
}

#[test]
fn knight_fork_needs_two_valuable_targets() {
    let host = host();
    let single = [Move { from: 1, to: 18, piece: "N", to_sq: "c3", attacks_valuable: 1, attacks: vec!["R"], ..Default::default() }];
    assert_eq!(
        picked(&host.call("knight_fork", "suggest", &[position(15, &single)]).unwrap()),
        None,
        "one target is not a fork"
    );

    let fork = [Move { from: 1, to: 20, piece: "N", to_sq: "e3", attacks_valuable: 2, attacks: vec!["Q", "R"], ..Default::default() }];
    assert_eq!(
        picked(&host.call("knight_fork", "suggest", &[position(15, &fork)]).unwrap()),
        Some((1, 20)),
        "two valuable targets is a fork"
    );
}

#[test]
fn castle_safety_castles_when_it_can() {
    let host = host();
    let moves = [
        Move { from: 8, to: 16, ..Default::default() },
        Move { from: 4, to: 6, piece: "K", to_sq: "g1", is_castle: true, ..Default::default() },
    ];
    let picked = picked(&host.call("castle_safety", "suggest", &[position(8, &moves)]).unwrap());
    assert_eq!(picked, Some((4, 6)));
}

#[test]
fn center_control_pushes_a_central_pawn_early_and_stays_quiet_late() {
    let host = host();
    let moves = [
        Move { from: 8, to: 16, to_sq: "a3", ..Default::default() },
        Move { from: 12, to: 28, to_sq: "e4", ..Default::default() },
    ];
    assert_eq!(
        picked(&host.call("center_control", "suggest", &[position(1, &moves)]).unwrap()),
        Some((12, 28)),
        "in the opening it takes the centre"
    );
    assert_eq!(
        picked(&host.call("center_control", "suggest", &[position(20, &moves)]).unwrap()),
        None,
        "past its window it declines"
    );
}

#[test]
fn develop_pieces_brings_a_minor_off_the_back_rank() {
    let host = host();
    let moves = [
        Move { from: 12, to: 28, to_sq: "e4", ..Default::default() },
        Move { from: 6, to: 21, piece: "N", from_sq: "g1", to_sq: "f3", ..Default::default() },
    ];
    let picked = picked(&host.call("develop_pieces", "suggest", &[position(3, &moves)]).unwrap());
    assert_eq!(picked, Some((6, 21)), "develop the knight");
}
