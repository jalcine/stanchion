## The rules of chess, in Godot.
##
## This is the half the design keeps *in the engine*: every piece's movement,
## castling, en passant, promotion, check and checkmate. Stanchion plugins never
## re-derive any of it — they only choose among the legal moves this class hands them,
## each already annotated with what it does (a capture, a check, a castle, …).
##
## Squares are indexed 0..63 as `rank * 8 + file`, with rank 0 the white back rank
## (a1..h1) and file 0 the a-file. Pieces are two-character codes like "wP" or "bK";
## an empty square is the empty string.
extends RefCounted

## Centipawn-ish worth of each piece type, used to rank captures. The king is scored
## high so "win the king" (i.e. checkmate lines) always outranks material.
const VALUE := {"P": 1, "N": 3, "B": 3, "R": 5, "Q": 9, "K": 1000}

var squares: Array[String] = []
var side_to_move := "w"
var castling := {"wk": true, "wq": true, "bk": true, "bq": true}
var en_passant := -1  # target square a pawn may capture onto, or -1
var fullmove := 1
## The moves played so far in SAN ("e4", "Nf3", "O-O"…), so opening plugins can match
## the game against a book line. Only the live board keeps this; trial clones do not.
var history_san: Array[String] = []

func _init() -> void:
	setup_start()

## Resets to the standard starting position.
func setup_start() -> void:
	squares = []
	squares.resize(64)
	for i in 64:
		squares[i] = ""
	var back := ["R", "N", "B", "Q", "K", "B", "N", "R"]
	for file in 8:
		squares[file] = "w" + back[file]
		squares[8 + file] = "wP"
		squares[48 + file] = "bP"
		squares[56 + file] = "b" + back[file]
	side_to_move = "w"
	castling = {"wk": true, "wq": true, "bk": true, "bq": true}
	en_passant = -1
	fullmove = 1
	history_san = []

## A deep copy, so a move can be tried without disturbing the live game.
func clone():
	var other = get_script().new()
	other.squares = squares.duplicate()
	other.side_to_move = side_to_move
	other.castling = castling.duplicate()
	other.en_passant = en_passant
	other.fullmove = fullmove
	return other

static func file_of(sq: int) -> int:
	return sq % 8

static func rank_of(sq: int) -> int:
	@warning_ignore("integer_division")
	return sq / 8

static func in_board(file: int, rank: int) -> bool:
	return file >= 0 and file < 8 and rank >= 0 and rank < 8

func color_of(code: String) -> String:
	return code.substr(0, 1) if code != "" else ""

func type_of(code: String) -> String:
	return code.substr(1, 1) if code != "" else ""

func opponent(color: String) -> String:
	return "b" if color == "w" else "w"

func king_square(color: String) -> int:
	var target := color + "K"
	for sq in 64:
		if squares[sq] == target:
			return sq
	return -1

# ---- attack detection -------------------------------------------------------

## Squares holding `by_color` pieces that attack `sq`. Single source behind
## is_attacked and the defender-graph annotations.
func _attackers_of(sq: int, by_color: String) -> Array:
	var out: Array = []
	var tf := file_of(sq)
	var tr := rank_of(sq)

	# Pawns attack diagonally forward, so an attacker sits one rank *behind* the
	# direction it pushes.
	var pawn_rank := tr - 1 if by_color == "w" else tr + 1
	for df in [-1, 1]:
		if in_board(tf + df, pawn_rank):
			var psq = pawn_rank * 8 + tf + df
			if squares[psq] == by_color + "P":
				out.append(psq)

	const KNIGHT := [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]]
	for step in KNIGHT:
		if in_board(tf + step[0], tr + step[1]):
			var nsq = (tr + step[1]) * 8 + tf + step[0]
			if squares[nsq] == by_color + "N":
				out.append(nsq)

	const KING := [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]]
	for step in KING:
		if in_board(tf + step[0], tr + step[1]):
			var ksq = (tr + step[1]) * 8 + tf + step[0]
			if squares[ksq] == by_color + "K":
				out.append(ksq)

	# Sliding pieces: rays until the board edge or the first piece.
	const ROOK_DIRS := [[1, 0], [-1, 0], [0, 1], [0, -1]]
	_collect_ray_attackers(tf, tr, ROOK_DIRS, by_color, ["R", "Q"], out)
	const BISHOP_DIRS := [[1, 1], [1, -1], [-1, 1], [-1, -1]]
	_collect_ray_attackers(tf, tr, BISHOP_DIRS, by_color, ["B", "Q"], out)
	return out

func _collect_ray_attackers(tf: int, tr: int, dirs: Array, by_color: String, types: Array, out: Array) -> void:
	for dir in dirs:
		var f: int = tf + int(dir[0])
		var r: int = tr + int(dir[1])
		while in_board(f, r):
			var code := squares[r * 8 + f]
			if code != "":
				if color_of(code) == by_color and types.has(type_of(code)):
					out.append(r * 8 + f)
				break
			f += int(dir[0])
			r += int(dir[1])

## Whether `by_color` attacks `sq` (used for check and castling-through-check).
func is_attacked(sq: int, by_color: String) -> bool:
	return not _attackers_of(sq, by_color).is_empty()

func in_check(color: String) -> bool:
	var ks := king_square(color)
	return ks != -1 and is_attacked(ks, opponent(color))

# ---- move generation --------------------------------------------------------

## Every legal move for `color`, each a Dictionary with `from`, `to` and flags. Moves
## that would leave the mover's own king in check are filtered out.
func legal_moves(color: String) -> Array:
	var legal: Array = []
	for move in _pseudo_moves(color):
		var trial = clone()
		trial._make(move)
		if not trial.in_check(color):
			legal.append(move)
	return legal

func _pseudo_moves(color: String) -> Array:
	var moves: Array = []
	for sq in 64:
		var code := squares[sq]
		if code == "" or color_of(code) != color:
			continue
		match type_of(code):
			"P": _pawn_moves(sq, color, moves)
			"N": _step_moves(sq, color, [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]], moves)
			"K": _king_moves(sq, color, moves)
			"B": _slide_moves(sq, color, [[1, 1], [1, -1], [-1, 1], [-1, -1]], moves)
			"R": _slide_moves(sq, color, [[1, 0], [-1, 0], [0, 1], [0, -1]], moves)
			"Q": _slide_moves(sq, color, [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]], moves)
	return moves

func _add(moves: Array, from: int, to: int, extra: Dictionary = {}) -> void:
	var move := {"from": from, "to": to, "promote": "", "castle": "", "en_passant": false}
	move.merge(extra, true)
	moves.append(move)

func _pawn_moves(sq: int, color: String, moves: Array) -> void:
	var dir := 1 if color == "w" else -1
	var start_rank := 1 if color == "w" else 6
	var last_rank := 7 if color == "w" else 0
	var f := file_of(sq)
	var r := rank_of(sq)

	# Forward one, and two from the starting rank.
	if in_board(f, r + dir) and squares[(r + dir) * 8 + f] == "":
		_push_pawn(moves, sq, (r + dir) * 8 + f, r + dir == last_rank)
		if r == start_rank and squares[(r + 2 * dir) * 8 + f] == "":
			_add(moves, sq, (r + 2 * dir) * 8 + f)

	# Captures, including en passant.
	for df in [-1, 1]:
		if not in_board(f + int(df), r + dir):
			continue
		var to: int = (r + dir) * 8 + f + int(df)
		var target := squares[to]
		if target != "" and color_of(target) == opponent(color):
			_push_pawn(moves, sq, to, r + dir == last_rank)
		elif to == en_passant:
			_add(moves, sq, to, {"en_passant": true})

func _push_pawn(moves: Array, from: int, to: int, promoting: bool) -> void:
	if promoting:
		for piece in ["Q", "R", "B", "N"]:
			_add(moves, from, to, {"promote": piece})
	else:
		_add(moves, from, to)

func _step_moves(sq: int, color: String, steps: Array, moves: Array) -> void:
	var f: int = file_of(sq)
	var r: int = rank_of(sq)
	for step in steps:
		if in_board(f + int(step[0]), r + int(step[1])):
			var to: int = (r + int(step[1])) * 8 + f + int(step[0])
			if squares[to] == "" or color_of(squares[to]) == opponent(color):
				_add(moves, sq, to)

func _slide_moves(sq: int, color: String, dirs: Array, moves: Array) -> void:
	var f := file_of(sq)
	var r := rank_of(sq)
	for dir in dirs:
		var nf: int = f + int(dir[0])
		var nr: int = r + int(dir[1])
		while in_board(nf, nr):
			var to: int = nr * 8 + nf
			if squares[to] == "":
				_add(moves, sq, to)
			else:
				if color_of(squares[to]) == opponent(color):
					_add(moves, sq, to)
				break
			nf += int(dir[0])
			nr += int(dir[1])

func _king_moves(sq: int, color: String, moves: Array) -> void:
	_step_moves(sq, color, [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]], moves)
	# Castling: rights intact, both squares empty, and the king neither starts in
	# check nor passes through an attacked square.
	var rank := 0 if color == "w" else 7
	if sq != rank * 8 + 4 or in_check(color):
		return
	var foe := opponent(color)
	if castling[color + "k"] and squares[rank * 8 + 5] == "" and squares[rank * 8 + 6] == "":
		if not is_attacked(rank * 8 + 5, foe) and not is_attacked(rank * 8 + 6, foe):
			_add(moves, sq, rank * 8 + 6, {"castle": "k"})
	if castling[color + "q"] and squares[rank * 8 + 3] == "" and squares[rank * 8 + 2] == "" and squares[rank * 8 + 1] == "":
		if not is_attacked(rank * 8 + 3, foe) and not is_attacked(rank * 8 + 2, foe):
			_add(moves, sq, rank * 8 + 2, {"castle": "q"})

# ---- applying moves ---------------------------------------------------------

## Applies a move to this board (used both on the live board and on trial clones).
func _make(move: Dictionary) -> void:
	var from: int = move["from"]
	var to: int = move["to"]
	var code := squares[from]
	var color := color_of(code)
	var rank := rank_of(from)

	# En passant removes the pawn behind the target square, not on it.
	if move.get("en_passant", false):
		var captured_sq := to - 8 if color == "w" else to + 8
		squares[captured_sq] = ""

	squares[to] = code
	squares[from] = ""

	if move.get("promote", "") != "":
		squares[to] = color + move["promote"]

	# Castling drags the rook along.
	if move.get("castle", "") == "k":
		squares[rank * 8 + 5] = squares[rank * 8 + 7]
		squares[rank * 8 + 7] = ""
	elif move.get("castle", "") == "q":
		squares[rank * 8 + 3] = squares[rank * 8]
		squares[rank * 8] = ""

	# A new en passant target only when a pawn just jumped two.
	en_passant = -1
	if type_of(code) == "P" and abs(rank_of(to) - rank) == 2:
		en_passant = (from + to) / 2

	_revoke_castling(from, to, color)
	if color == "b":
		fullmove += 1
	side_to_move = opponent(color)

func _revoke_castling(from: int, to: int, color: String) -> void:
	if type_of(squares[to]) == "K" or (from == king_square(color) and squares[to] == color + "K"):
		castling[color + "k"] = false
		castling[color + "q"] = false
	# Rook moved or was captured off its home square.
	for sq in [from, to]:
		match sq:
			0: castling["wq"] = false
			7: castling["wk"] = false
			56: castling["bq"] = false
			63: castling["bk"] = false

## Public entry point: apply a move to the live board, recording its SAN.
func apply(move: Dictionary) -> void:
	history_san.append(_san(move, legal_moves(side_to_move)))
	_make(move)


## Standard algebraic notation for a move, given the side-to-move's full legal list for
## disambiguation. Computed on the pre-move board.
func _san(move: Dictionary, legal: Array) -> String:
	if move.get("castle", "") == "k":
		return _san_suffix(move, "O-O")
	if move.get("castle", "") == "q":
		return _san_suffix(move, "O-O-O")
	var from: int = move["from"]
	var to: int = move["to"]
	var piece := type_of(squares[from])
	var is_capture: bool = squares[to] != "" or move.get("en_passant", false)
	var base := ""
	if piece == "P":
		if is_capture:
			base = "abcdefgh"[file_of(from)] + "x"
		base += square_name(to)
		if move.get("promote", "") != "":
			base += "=" + move["promote"]
	else:
		# Disambiguate against other same-type pieces that can also reach `to`.
		var same_file := false
		var same_rank := false
		var clash := false
		for m in legal:
			if m["from"] != from and m["to"] == to and type_of(squares[m["from"]]) == piece:
				clash = true
				if file_of(m["from"]) == file_of(from):
					same_file = true
				if rank_of(m["from"]) == rank_of(from):
					same_rank = true
		var disamb := ""
		if clash:
			if not same_file:
				disamb = "abcdefgh"[file_of(from)]
			elif not same_rank:
				disamb = str(rank_of(from) + 1)
			else:
				disamb = square_name(from)
		base = piece + disamb + ("x" if is_capture else "") + square_name(to)
	return _san_suffix(move, base)


## Appends "+" for check or "#" for mate to a SAN string.
func _san_suffix(move: Dictionary, base: String) -> String:
	var mover := color_of(squares[move["from"]])
	var foe := opponent(mover)
	var trial = clone()
	trial._make(move)
	if trial.in_check(foe):
		return base + ("#" if trial.legal_moves(foe).is_empty() else "+")
	return base

# ---- annotation & position export ------------------------------------------

## The current side's legal moves, each annotated with what it *does* — the shape the
## Lua combination plugins reason over.
func annotated_moves(color: String) -> Array:
	var out: Array = []
	var legal := legal_moves(color)
	for move in legal:
		var to: int = move["to"]
		var target := squares[to]
		var capture := target
		if move.get("en_passant", false):
			capture = opponent(color) + "P"
		var san := _san(move, legal)
		var trial = clone()
		trial._make(move)
		var foe := opponent(color)
		# What the moved piece attacks from its destination — the raw material a fork
		# or pin plugin reasons over.
		var landed: int = move["to"]
		var attacks = trial.attacked_targets(landed)
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
		var tactic = trial._slider_tactic(landed)
		# Defender-graph facts: what the mover is worth, whether the prize is
		# guarded, who recaptures on the destination, and double check.
		var mover_value: int = VALUE.get(type_of(squares[move["from"]]), 0)
		var captured_defended := not _attackers_of(to, foe).is_empty()
		var attackers_of_to = trial._attackers_of(landed, foe)
		var foe_king = trial.king_square(foe)
		var is_double = gives_check and foe_king != -1 and trial._attackers_of(foe_king, color).size() >= 2
		out.append({
			"attacks": attacks,
			"attacks_valuable": valuable,
			"from": move["from"],
			"to": to,
			"piece": type_of(squares[move["from"]]),
			"capture": type_of(capture),
			"captured_value": VALUE.get(type_of(capture), 0),
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
			"from_sq": square_name(move["from"]),
			"to_sq": square_name(to),
			"raw": move,
		})
	return out


## Looks at the sliding piece on `sq` (bishop/rook/queen) and reports whether it pins or
## skewers along any ray: two enemy pieces in a row with only empty squares between the
## slider and the first. A pin has the less valuable enemy in front (or its king behind);
## a skewer has the more valuable in front. Returns `{pin, skewer, value}` for the best
## such find (`value` is the material it threatens).
func _slider_tactic(sq: int) -> Dictionary:
	var result := {"pin": false, "skewer": false, "value": 0}
	var code := squares[sq]
	if code == "":
		return result
	var color := color_of(code)
	var foe := opponent(color)
	var dirs: Array
	match type_of(code):
		"B": dirs = [[1, 1], [1, -1], [-1, 1], [-1, -1]]
		"R": dirs = [[1, 0], [-1, 0], [0, 1], [0, -1]]
		"Q": dirs = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]]
		_: return result
	var f := file_of(sq)
	var r := rank_of(sq)
	for dir in dirs:
		var nf := f + int(dir[0])
		var nr := r + int(dir[1])
		# Walk to the first piece on the ray.
		while in_board(nf, nr) and squares[nr * 8 + nf] == "":
			nf += int(dir[0])
			nr += int(dir[1])
		if not in_board(nf, nr) or color_of(squares[nr * 8 + nf]) != foe:
			continue
		var front := type_of(squares[nr * 8 + nf])
		# Walk on to the next piece behind it.
		nf += int(dir[0])
		nr += int(dir[1])
		while in_board(nf, nr) and squares[nr * 8 + nf] == "":
			nf += int(dir[0])
			nr += int(dir[1])
		if not in_board(nf, nr) or color_of(squares[nr * 8 + nf]) != foe:
			continue
		var back := type_of(squares[nr * 8 + nf])
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
func attacked_targets(sq: int) -> Array:
	var code := squares[sq]
	if code == "":
		return []
	var color := color_of(code)
	var foe := opponent(color)
	var f := file_of(sq)
	var r := rank_of(sq)
	var hits: Array = []
	match type_of(code):
		"P":
			var dir := 1 if color == "w" else -1
			for df in [-1, 1]:
				if in_board(f + df, r + dir):
					var t := squares[(r + dir) * 8 + f + df]
					if t != "" and color_of(t) == foe:
						hits.append(type_of(t))
		"N":
			_collect_steps(f, r, [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]], foe, hits)
		"K":
			_collect_steps(f, r, [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]], foe, hits)
		"B":
			_collect_rays(f, r, [[1, 1], [1, -1], [-1, 1], [-1, -1]], foe, hits)
		"R":
			_collect_rays(f, r, [[1, 0], [-1, 0], [0, 1], [0, -1]], foe, hits)
		"Q":
			_collect_rays(f, r, [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]], foe, hits)
	return hits

func _collect_steps(f: int, r: int, steps: Array, foe: String, hits: Array) -> void:
	for step in steps:
		if in_board(f + step[0], r + step[1]):
			var t := squares[(r + step[1]) * 8 + f + step[0]]
			if t != "" and color_of(t) == foe:
				hits.append(type_of(t))

func _collect_rays(f: int, r: int, dirs: Array, foe: String, hits: Array) -> void:
	for dir in dirs:
		var nf: int = f + int(dir[0])
		var nr: int = r + int(dir[1])
		while in_board(nf, nr):
			var t := squares[nr * 8 + nf]
			if t != "":
				if color_of(t) == foe:
					hits.append(type_of(t))
				break
			nf += int(dir[0])
			nr += int(dir[1])

static func square_name(sq: int) -> String:
	return "abcdefgh"[sq % 8] + str(int(sq / 8) + 1)

func material(color: String) -> int:
	var total := 0
	for code in squares:
		if code != "" and color_of(code) == color and type_of(code) != "K":
			total += VALUE.get(type_of(code), 0)
	return total

## Squares each `by_color` piece attacks, as `{sq: [attacker_sqs]}`. Built once per
## position (not per move) — the defender graph plugins reason over.
func _attack_map(by_color: String) -> Dictionary:
	var map := {}
	for sq in 64:
		var hit := _attackers_of(sq, by_color)
		if not hit.is_empty():
			map[sq] = hit
	return map

## A plain Dictionary snapshot for a plugin: the board, whose move it is, material,
## check state and the annotated legal moves.
func position_for(color: String) -> Dictionary:
	return {
		"board": squares.duplicate(),
		"side": color,
		"fullmove": fullmove,
		"in_check": in_check(color),
		"material": {"w": material("w"), "b": material("b")},
		"history": history_san.duplicate(),
		"foe_attacks": _attack_map(opponent(color)),
		"own_attacks": _attack_map(color),
		"moves": annotated_moves(color),
	}
