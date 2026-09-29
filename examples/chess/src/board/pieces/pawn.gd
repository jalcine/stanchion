## Pawn movement: pushes, double push, captures (incl. en passant), promotions.
## Appends via b.add_move; reads b.squares and b.en_passant.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")

static func generate(b, sq: int, color: String, moves: Array) -> void:
	var dir := 1 if color == "w" else -1
	var start_rank := 1 if color == "w" else 6
	var last_rank := 7 if color == "w" else 0
	var f := Geo.file_of(sq)
	var r := Geo.rank_of(sq)

	# Forward one, and two from the starting rank.
	if Geo.in_board(f, r + dir) and b.squares[(r + dir) * 8 + f] == "":
		_push(b, moves, sq, (r + dir) * 8 + f, r + dir == last_rank)
		if r == start_rank and b.squares[(r + 2 * dir) * 8 + f] == "":
			b.add_move(moves, sq, (r + 2 * dir) * 8 + f)

	# Captures, including en passant.
	for df in [-1, 1]:
		if not Geo.in_board(f + int(df), r + dir):
			continue
		var to: int = (r + dir) * 8 + f + int(df)
		var target: String = b.squares[to]
		if target != "" and Geo.color_of(target) == Geo.opponent(color):
			_push(b, moves, sq, to, r + dir == last_rank)
		elif to == b.en_passant:
			b.add_move(moves, sq, to, {"en_passant": true})

static func _push(b, moves: Array, from: int, to: int, promoting: bool) -> void:
	if promoting:
		for piece in ["Q", "R", "B", "N"]:
			b.add_move(moves, from, to, {"promote": piece})
	else:
		b.add_move(moves, from, to)
