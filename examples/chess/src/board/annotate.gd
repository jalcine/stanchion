## Position export for plugins: annotated legal moves plus the defender graph.
## The shape Lua reasons over — the XRPC contract in `position_for`.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")
const Attacks = preload("res://src/board/attacks.gd")
const San = preload("res://src/board/san.gd")

## Centipawn-ish worth of each piece type, used to rank captures. The king is scored
## high so "win the king" (i.e. checkmate lines) always outranks material.
const VALUE := {"P": 1, "N": 3, "B": 3, "R": 5, "Q": 9, "K": 1000}

## The current side's legal moves, each annotated with what it *does*.
static func annotated_moves(b, color: String) -> Array:
	var out: Array = []
	var legal = b.legal_moves(color)
	for move in legal:
		var to: int = move["to"]
		var target: String = b.squares[to]
		var capture := target
		if move.get("en_passant", false):
			capture = Geo.opponent(color) + "P"
		var san := San.san(b, move, legal)
		var trial = b.clone()
		trial._make(move)
		var foe := Geo.opponent(color)
		# What the moved piece attacks from its destination — the raw material a fork
		# or pin plugin reasons over.
		var landed: int = move["to"]
		var attacks = attacked_targets(trial, landed)
		var valuable := 0
		for t in attacks:
			if VALUE.get(t, 0) >= 3:
				valuable += 1
		var gives_check = trial.in_check(foe)
		# Discovered check: the side gives check, but not with the piece that moved.
		var is_discovered = gives_check and not attacks.has("K")
		# Whether the destination is attacked by the opponent — i.e. the piece would hang
		# there. Tactics that leave the piece en prise are usually not worth it.
		var to_is_attacked = trial.is_attacked(landed, foe)
		# Pins and skewers created by a sliding piece landing here.
		var tactic = slider_tactic(trial, landed)
		# Defender-graph facts: what the mover is worth, whether the prize is
		# guarded, who recaptures on the destination, and double check.
		var mover_value: int = VALUE.get(Geo.type_of(b.squares[move["from"]]), 0)
		var captured_defended := not Attacks.attackers_of(b.squares, to, foe).is_empty()
		var attackers_of_to = Attacks.attackers_of(trial.squares, landed, foe)
		var foe_king = trial.king_square(foe)
		var is_double = gives_check and foe_king != -1 and Attacks.attackers_of(trial.squares, foe_king, color).size() >= 2
		out.append({
			"attacks": attacks,
			"attacks_valuable": valuable,
			"from": move["from"],
			"to": to,
			"piece": Geo.type_of(b.squares[move["from"]]),
			"capture": Geo.type_of(capture),
			"captured_value": VALUE.get(Geo.type_of(capture), 0),
			"is_castle": move.get("castle", "") != "",
			"is_en_passant": move.get("en_passant", false),
			"promotes": move.get("promote", ""),
			"gives_check": gives_check,
			"is_discovered_check": is_discovered,
			"is_mate": gives_check and trial.legal_moves(foe).is_empty(),
			"creates_pin": tactic["pin"],
			"creates_skewer": tactic["skewer"],
			"tactic_value": tactic["value"],
			"to_is_attacked": to_is_attacked,
			"mover_value": mover_value,
			"captured_defended": captured_defended,
			"attackers_of_to": attackers_of_to,
			"is_double_check": is_double,
			"san": san,
			"from_sq": Geo.square_name(move["from"]),
			"to_sq": Geo.square_name(to),
			"raw": move,
		})
	return out

## Looks at the sliding piece on `sq` (bishop/rook/queen) and reports whether it pins or
## skewers along any ray: two enemy pieces in a row with only empty squares between the
## slider and the first. Returns `{pin, skewer, value}` for the best such find.
static func slider_tactic(b, sq: int) -> Dictionary:
	var result := {"pin": false, "skewer": false, "value": 0}
	var code: String = b.squares[sq]
	if code == "":
		return result
	var color := Geo.color_of(code)
	var foe := Geo.opponent(color)
	var dirs: Array
	match Geo.type_of(code):
		"B": dirs = [[1, 1], [1, -1], [-1, 1], [-1, -1]]
		"R": dirs = [[1, 0], [-1, 0], [0, 1], [0, -1]]
		"Q": dirs = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]]
		_: return result
	var f := Geo.file_of(sq)
	var r := Geo.rank_of(sq)
	for dir in dirs:
		var nf := f + int(dir[0])
		var nr := r + int(dir[1])
		# Walk to the first piece on the ray.
		while Geo.in_board(nf, nr) and b.squares[nr * 8 + nf] == "":
			nf += int(dir[0])
			nr += int(dir[1])
		if not Geo.in_board(nf, nr) or Geo.color_of(b.squares[nr * 8 + nf]) != foe:
			continue
		var front := Geo.type_of(b.squares[nr * 8 + nf])
		# Walk on to the next piece behind it.
		nf += int(dir[0])
		nr += int(dir[1])
		while Geo.in_board(nf, nr) and b.squares[nr * 8 + nf] == "":
			nf += int(dir[0])
			nr += int(dir[1])
		if not Geo.in_board(nf, nr) or Geo.color_of(b.squares[nr * 8 + nf]) != foe:
			continue
		var back := Geo.type_of(b.squares[nr * 8 + nf])
		var front_v: int = VALUE.get(front, 0)
		var back_v: int = VALUE.get(back, 0)
		# King in front, or a more valuable front piece → skewer (front must move, we win
		# the piece behind). Otherwise the front piece is pinned to the more valuable one.
		# We only report tactics that win a real piece (a minor or better), so a queen
		# does not go chasing a pinned pawn.
		if front == "K" or front_v > back_v:
			if back_v >= 3 and back_v > result["value"]:
				result = {"pin": false, "skewer": true, "value": back_v}
		else:
			if front_v >= 3 and front_v > result["value"]:
				result = {"pin": true, "skewer": false, "value": front_v}
	return result

## The enemy piece types a piece standing on `sq` attacks on the current board — the
## rule a "knight fork" plugin needs without re-deriving movement itself. Returns an
## Array of type codes like ["Q", "R"].
static func attacked_targets(b, sq: int) -> Array:
	var code: String = b.squares[sq]
	if code == "":
		return []
	var color := Geo.color_of(code)
	var foe := Geo.opponent(color)
	var f := Geo.file_of(sq)
	var r := Geo.rank_of(sq)
	var hits: Array = []
	match Geo.type_of(code):
		"P":
			var dir := 1 if color == "w" else -1
			for df in [-1, 1]:
				if Geo.in_board(f + df, r + dir):
					var t: String = b.squares[(r + dir) * 8 + f + df]
					if t != "" and Geo.color_of(t) == foe:
						hits.append(Geo.type_of(t))
		"N":
			_collect_steps(b, f, r, [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]], foe, hits)
		"K":
			_collect_steps(b, f, r, [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]], foe, hits)
		"B":
			_collect_rays(b, f, r, [[1, 1], [1, -1], [-1, 1], [-1, -1]], foe, hits)
		"R":
			_collect_rays(b, f, r, [[1, 0], [-1, 0], [0, 1], [0, -1]], foe, hits)
		"Q":
			_collect_rays(b, f, r, [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]], foe, hits)
	return hits

static func _collect_steps(b, f: int, r: int, steps: Array, foe: String, hits: Array) -> void:
	for step in steps:
		if Geo.in_board(f + step[0], r + step[1]):
			var t: String = b.squares[(r + step[1]) * 8 + f + step[0]]
			if t != "" and Geo.color_of(t) == foe:
				hits.append(Geo.type_of(t))

static func _collect_rays(b, f: int, r: int, dirs: Array, foe: String, hits: Array) -> void:
	for dir in dirs:
		var nf: int = f + int(dir[0])
		var nr: int = r + int(dir[1])
		while Geo.in_board(nf, nr):
			var t: String = b.squares[nr * 8 + nf]
			if t != "":
				if Geo.color_of(t) == foe:
					hits.append(Geo.type_of(t))
				break
			nf += int(dir[0])
			nr += int(dir[1])

static func material(b, color: String) -> int:
	var total := 0
	for code in b.squares:
		if code != "" and Geo.color_of(code) == color and Geo.type_of(code) != "K":
			total += VALUE.get(Geo.type_of(code), 0)
	return total

## A plain Dictionary snapshot for a plugin: the board, whose move it is, material,
## check state and the annotated legal moves.
static func position_for(b, color: String) -> Dictionary:
	return {
		"board": b.squares.duplicate(),
		"side": color,
		"fullmove": b.fullmove,
		"in_check": b.in_check(color),
		"material": {"w": material(b, "w"), "b": material(b, "b")},
		"history": b.history_san.duplicate(),
		"foe_attacks": Attacks.attack_map(b.squares, Geo.opponent(color)),
		"own_attacks": Attacks.attack_map(b.squares, color),
		"moves": annotated_moves(b, color),
	}
