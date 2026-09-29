## Move dispatcher: one piece class per type. Board-agnostic except for the
## squares it reads through b.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")
const Pawn = preload("res://src/board/pieces/pawn.gd")
const Steps = preload("res://src/board/pieces/steps.gd")
const Sliders = preload("res://src/board/pieces/sliders.gd")
const King = preload("res://src/board/pieces/king.gd")

const KNIGHT_STEPS := [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]]
const BISHOP_DIRS := [[1, 1], [1, -1], [-1, 1], [-1, -1]]
const ROOK_DIRS := [[1, 0], [-1, 0], [0, 1], [0, -1]]
const QUEEN_DIRS := [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]]


static func pseudo(b, color: String) -> Array:
	var moves: Array = []
	for sq in 64:
		var code: String = b.squares[sq]
		if code == "" or Geo.color_of(code) != color:
			continue
		match Geo.type_of(code):
			"P":
				Pawn.generate(b, sq, color, moves)
			"N":
				Steps.generate(b, sq, color, moves, KNIGHT_STEPS)
			"K":
				King.generate(b, sq, color, moves)
			"B":
				Sliders.generate(b, sq, color, moves, BISHOP_DIRS)
			"R":
				Sliders.generate(b, sq, color, moves, ROOK_DIRS)
			"Q":
				Sliders.generate(b, sq, color, moves, QUEEN_DIRS)
	return moves
