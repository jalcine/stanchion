## Attack questions over a squares array: who attacks a square, and the full
## per-position defender graph. Pure functions — no board reference.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")

const KNIGHT := [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]]
const KING := [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]]
const ROOK_DIRS := [[1, 0], [-1, 0], [0, 1], [0, -1]]
const BISHOP_DIRS := [[1, 1], [1, -1], [-1, 1], [-1, -1]]


## Squares holding `by_color` pieces that attack `sq`.
static func attackers_of(squares: Array, sq: int, by_color: String) -> Array:
	var out: Array = []
	var tf := Geo.file_of(sq)
	var tr := Geo.rank_of(sq)

	# Pawns attack diagonally forward, so an attacker sits one rank *behind* the
	# direction it pushes.
	var pawn_rank := tr - 1 if by_color == "w" else tr + 1
	for df in [-1, 1]:
		if Geo.in_board(tf + df, pawn_rank):
			var psq = pawn_rank * 8 + tf + df
			if squares[psq] == by_color + "P":
				out.append(psq)

	for step in KNIGHT:
		if Geo.in_board(tf + step[0], tr + step[1]):
			var nsq = (tr + step[1]) * 8 + tf + step[0]
			if squares[nsq] == by_color + "N":
				out.append(nsq)

	for step in KING:
		if Geo.in_board(tf + step[0], tr + step[1]):
			var ksq = (tr + step[1]) * 8 + tf + step[0]
			if squares[ksq] == by_color + "K":
				out.append(ksq)

	# Sliding pieces: rays until the board edge or the first piece.
	_collect_rays(squares, tf, tr, ROOK_DIRS, by_color, ["R", "Q"], out)
	_collect_rays(squares, tf, tr, BISHOP_DIRS, by_color, ["B", "Q"], out)
	return out


static func _collect_rays(
	squares: Array, tf: int, tr: int, dirs: Array, by_color: String, types: Array, out: Array
) -> void:
	for dir in dirs:
		var f: int = tf + int(dir[0])
		var r: int = tr + int(dir[1])
		while Geo.in_board(f, r):
			var code: String = squares[r * 8 + f]
			if code != "":
				if Geo.color_of(code) == by_color and types.has(Geo.type_of(code)):
					out.append(r * 8 + f)
				break
			f += int(dir[0])
			r += int(dir[1])


static func is_attacked(squares: Array, sq: int, by_color: String) -> bool:
	return not attackers_of(squares, sq, by_color).is_empty()


## `{sq: [attacker_sqs]}` for one color. Built once per position, not per move.
static func attack_map(squares: Array, by_color: String) -> Dictionary:
	var map := {}
	for sq in 64:
		var hit := attackers_of(squares, sq, by_color)
		if not hit.is_empty():
			map[sq] = hit
	return map
