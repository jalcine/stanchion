# Chess combinations expansion — spec

## Goal

50 combination plugins total (10 existing + 40 new). Engine-first: extend the
`position_for` XRPC contract with a defender graph, then write all 40 against it.

## XRPC contract (Stanchion `dispatch("suggest", [position])`)

`position` (built once per dispatch in `position_for`):
- existing: `board` (64 codes), `side`, `fullmove`, `in_check`, `material`,
  `history` (SAN), `moves` (annotated).
- NEW `foe_attacks`: `{sq: [attacker_sqs]}` — foe pieces attacking each square, pre-move.
- NEW `own_attacks`: same for own color.
- Int keys arrive in Lua as numeric keys; missing key = nil → plugins use
  `position.foe_attacks or {}` and `(map[sq] or {})`.

Annotated move additions (`annotated_moves`, per legal move):
- `mover_value: int` — VALUE of the moving piece.
- `captured_defended: bool` — pre-move foe attackers of destination non-empty.
- `attackers_of_to: Array[int]` — post-move (`trial`) foe attackers of destination.
- `is_double_check: bool` — check with ≥2 distinct checkers post-move.

Unchanged: `attacks`, `attacks_valuable`, `capture`, `captured_value`, `gives_check`,
`is_discovered_check`, `is_mate`, `creates_pin`, `creates_skewer`, `tactic_value`,
`to_is_attacked`, `is_castle`, `is_en_passant`, `promotes`, `san`, `from/to`,
`from_sq/to_sq`, `piece`, `raw`.

## Engine source of truth

`is_attacked(sq, color)` is refactored onto one enumerator
`_attackers_of(sq, by_color) -> Array[int]` (pawn/knight/king offsets + ray walk).
`is_attacked` becomes `not _attackers_of(...).is_empty()`.
Excluded (need search, not data): zwischenzug, perpetual check, windmill sequences.

## Strength ladder (no inversions)

checkmate 1000 > mate patterns 900 > double_check 700 > royal_fork 620 >
discovered_check ~560+ > removal 540 > sacrifice 530 > deflection 520 >
overloading 510 > decoy/interference/knight_fork ~500 > x_ray 490 >
pawn/bishop/rook/queen forks 480 > cross_check 480 > underpromotion 470 >
battery 470 > check_and_gain 460 > promotion 450+ > back_rank_threat 440 >
desperado 420 > king_flight 400 > en_passant 350 > hanging 340+ > trade 330+ >
win_material 300+ / openings 300 / passed_push 300 / outpost 300 > rook_seventh 280 >
simplify 260 > rook_file 240 > pawn_break 220 > castle/activate_king 200 >
avoid_trade 180.

## The 40 (folder → Name → trigger)

Mate patterns (is_mate + geometry via board scan):
1. `back_rank_mate` Back Rank Mate — R/Q, victim back rank.
2. `smothered_mate` Smothered Mate — N, all escapes edge/own-piece (else nil).
3. `arabian_mate` Arabian Mate — R/N, victim king corner.
4. `anastasia_mate` Anastasia's Mate — R/N, edge file non-corner.
5. `dovetail_mate` Dovetail Mate — Q, edge king.
6. `bodens_mate` Boden's Mate — B, both own bishops alive.
7. `hook_mate` Hook Mate — N/R, edge king + own pawn wedge (scan, else nil).
8. `kill_box_mate` Kill Box Mate — edge king, other patterns silent.
Forks: 9. `pawn_fork` (P,≥2 valuable) 10. `bishop_fork` 11. `rook_fork`
12. `queen_fork` 13. `royal_fork` (attacks has K + ≥1 valuable).
Graph (needs new fields): 14. `removal_of_defender` (captured_defended +
attackers_of_to empty + value≥3) 15. `deflection` (gives_check + defended
capture ≥3, not mate) 16. `decoy` (mover≥3 onto attacked sq + attacks≥1)
17. `overloading` (attacks_valuable≥2 + captured_defended)
18. `interference` (#foe_attacks[to]≥2 + destination safe post-move)
19. `x_ray_attack` (slider + creates_pin + attacks_valuable≥1)
20. `battery` (forms aligned slider battery + threat) 21. `sacrifice`
(mover−captured ≥2 + check/attack tempo).
Forcing: 22. `double_check` 23. `check_and_gain` 24. `cross_check`
(in_check + gives_check).
Material: 25. `hanging_piece` (capture undefended) 26. `trade_winner`
(captured−mover>0) 27. `simplify_ahead` (up ≥5, trade down) 28. `avoid_trade_behind`
(down ≥3, quiet safe move) 29. `desperado` (attacked own piece moves with gain).
Pawns: 30. `pawn_promotion` 31. `passed_pawn_push` 32. `pawn_break`
33. `knight_underpromotion`.
Positional: 34. `rook_open_file` 35. `rook_seventh` 36. `outpost_knight`
37. `activate_king` (thin endgame, king centralizes).
King safety: 38. `back_rank_threat` 39. `king_flight` (in_check, safe K move).
40. `en_passant_theme`.

## Difficulties (main.gd DIFFICULTY)

- Gentle: + `pawn_promotion`, `hanging_piece`.
- Steady: + `trade_winner`, `hanging_piece`, `pawn_promotion`, `passed_pawn_push`,
  `rook_open_file`, `check_and_gain`, `desperado`, `king_flight`,
  `back_rank_threat`, `en_passant_theme`, `avoid_trade_behind`.
- Sharp: all 50.

## Verify

`godot --headless --import`, then `timeout 3s mise chess` → log shows 50 loaded,
zero SCRIPT ERRORs. In-game Reload picks up folders live.
