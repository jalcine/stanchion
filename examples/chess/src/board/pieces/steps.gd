## Fixed-offset movers: knights and (non-castling) kings.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")


static func generate(b, sq: int, color: String, moves: Array, steps: Array) -> void:
	var f: int = Geo.file_of(sq)
	var r: int = Geo.rank_of(sq)
	for step in steps:
		if Geo.in_board(f + int(step[0]), r + int(step[1])):
			var to: int = (r + int(step[1])) * 8 + f + int(step[0])
			if b.squares[to] == "" or Geo.color_of(b.squares[to]) == Geo.opponent(color):
				b.add_move(moves, sq, to)
