## Standard algebraic notation. Computed on the pre-move board, given the
## side-to-move's full legal list for disambiguation.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")

static func san(b, move: Dictionary, legal: Array) -> String:
	if move.get("castle", "") == "k":
		return suffix(b, move, "O-O")
	if move.get("castle", "") == "q":
		return suffix(b, move, "O-O-O")
	var from: int = move["from"]
	var to: int = move["to"]
	var piece := Geo.type_of(b.squares[from])
	var is_capture: bool = b.squares[to] != "" or move.get("en_passant", false)
	var base := ""
	if piece == "P":
		if is_capture:
			base = "abcdefgh"[Geo.file_of(from)] + "x"
		base += Geo.square_name(to)
		if move.get("promote", "") != "":
			base += "=" + move["promote"]
	else:
		# Disambiguate against other same-type pieces that can also reach `to`.
		var same_file := false
		var same_rank := false
		var clash := false
		for m in legal:
			if m["from"] != from and m["to"] == to and Geo.type_of(b.squares[m["from"]]) == piece:
				clash = true
				if Geo.file_of(m["from"]) == Geo.file_of(from):
					same_file = true
				if Geo.rank_of(m["from"]) == Geo.rank_of(from):
					same_rank = true
		var disamb := ""
		if clash:
			if not same_file:
				disamb = "abcdefgh"[Geo.file_of(from)]
			elif not same_rank:
				disamb = str(Geo.rank_of(from) + 1)
			else:
				disamb = Geo.square_name(from)
		base = piece + disamb + ("x" if is_capture else "") + Geo.square_name(to)
	return suffix(b, move, base)

## Appends "+" for check or "#" for mate to a SAN string.
static func suffix(b, move: Dictionary, base: String) -> String:
	var mover := Geo.color_of(b.squares[move["from"]])
	var foe := Geo.opponent(mover)
	var trial = b.clone()
	trial._make(move)
	if trial.in_check(foe):
		return base + ("#" if trial.legal_moves(foe).is_empty() else "+")
	return base
