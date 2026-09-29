## King movement: one step anywhere plus castling. Castling needs rights, empty
## transit squares, and a king that neither starts in nor passes through check.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")
const Steps = preload("res://src/board/pieces/steps.gd")

const KING_STEPS := [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]]


static func generate(b, sq: int, color: String, moves: Array) -> void:
	Steps.generate(b, sq, color, moves, KING_STEPS)
	var rank := 0 if color == "w" else 7
	if sq != rank * 8 + 4 or b.in_check(color):
		return
	var foe := Geo.opponent(color)
	if b.castling[color + "k"] and b.squares[rank * 8 + 5] == "" and b.squares[rank * 8 + 6] == "":
		if not b.is_attacked(rank * 8 + 5, foe) and not b.is_attacked(rank * 8 + 6, foe):
			b.add_move(moves, sq, rank * 8 + 6, {"castle": "k"})
	if (
		b.castling[color + "q"]
		and b.squares[rank * 8 + 3] == ""
		and b.squares[rank * 8 + 2] == ""
		and b.squares[rank * 8 + 1] == ""
	):
		if not b.is_attacked(rank * 8 + 3, foe) and not b.is_attacked(rank * 8 + 2, foe):
			b.add_move(moves, sq, rank * 8 + 2, {"castle": "q"})
