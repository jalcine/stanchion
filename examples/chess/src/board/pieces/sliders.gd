## Ray movers: bishops, rooks, queens. Walks each ray to the edge or blocker.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")

static func generate(b, sq: int, color: String, moves: Array, dirs: Array) -> void:
	var f := Geo.file_of(sq)
	var r := Geo.rank_of(sq)
	for dir in dirs:
		var nf: int = f + int(dir[0])
		var nr: int = r + int(dir[1])
		while Geo.in_board(nf, nr):
			var to: int = nr * 8 + nf
			if b.squares[to] == "":
				b.add_move(moves, sq, to)
			else:
				if Geo.color_of(b.squares[to]) == Geo.opponent(color):
					b.add_move(moves, sq, to)
				break
			nf += int(dir[0])
			nr += int(dir[1])
