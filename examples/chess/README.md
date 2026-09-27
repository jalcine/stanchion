# Stanchion Chess

A small chess game that splits its brain in two:

- **The rules live in Godot.** `src/board.gd` is a complete rules engine — every
  piece's movement, castling, en passant, promotion, check, checkmate and stalemate.
  It also *annotates* each legal move with what it does: whether it captures (and what
  it is worth), gives check, is mate, castles, or attacks a valuable piece after
  moving.

- **The plays live in Lua, loaded through Stanchion.** Each folder under `plugins/` is
  a sandboxed [stanchion](../../README.md) plugin describing one *combination* —
  mate-in-one, win material, knight fork, castle for safety, control the centre,
  develop your pieces. A plugin never re-derives the rules; it only reads the annotated
  position Godot hands it and proposes a move.

The same catalogue does double duty:

- The **AI** asks every combination for its move and plays the strongest one that
  applies. *Which* combinations it is allowed to consult sets the difficulty — Gentle
  sees only the positional ideas, Sharp sees the tactics and mates too.
- The **player** is shown every combination available to them as a live hint, with the
  top suggestion drawn as an arrow on the board.

Adding a new opening idea or tactic is dropping a new folder in `plugins/` — no
recompiling the game, and the plugin runs sandboxed with a bounded instruction budget.

## Running it

1. Build the GDExtension from the repository root:

   ```sh
   cargo build -p stanchion-godot
   ```

2. Open this folder (`examples/chess`) as a project in **Godot 4.2+** and press play.
   `stanchion.gdextension` points at `target/debug/libstanchion_godot.*`; for an
   exported build, `cargo build --release -p stanchion-godot` and ship the release
   library instead.

If the extension is missing, the game still runs the rules — you just get no AI plays
or hints, and a note explaining why.

## The plugin contract

Each combination is a Lua class with a `suggest(position)` method. `position` is a
table shaped by `ChessBoard.position_for`:

```
{
  side = "w" | "b",
  fullmove = <int>,
  in_check = <bool>,
  material = { w = <int>, b = <int> },
  board = [ 64 piece codes: "wP", "bK", "" … ],
  moves = [               -- every legal move, annotated
    {
      from = <0..63>, to = <0..63>,
      piece = "P".."K", from_sq = "e2", to_sq = "e4",
      capture = "" | "P".."Q", captured_value = <int>,
      is_castle = <bool>, is_en_passant = <bool>, promotes = "" | "Q",
      gives_check = <bool>, is_mate = <bool>,
      attacks = [ enemy piece types hit from the destination ],
      attacks_valuable = <int>,   -- how many of those are worth a minor piece or more
    }, …
  ],
}
```

`suggest` returns `nil` (the combination does not apply) or:

```lua
{ from = <int>, to = <int>, promote = "", strength = <int>, name = "…", rationale = "…" }
```

Godot re-checks the returned `from`/`to` against its own legal moves before playing
it, so a misbehaving plugin can never make an illegal move — the sandbox bounds what it
can *do*, and the rules engine bounds what it can *play*.

The Lua-side logic is covered by `bindings/godot/tests/combinations.rs`, which loads
these very plugins through the runtime and checks each pick.

## Assets

Board and cursor art are from the **pixelCheckers** pack by **DANI MACCARI** — see
[`assets/CREDITS.md`](assets/CREDITS.md). Chess pieces are Unicode glyphs from Godot's
default font.
