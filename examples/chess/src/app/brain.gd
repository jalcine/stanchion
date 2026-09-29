## AI move choice over plugin suggestions: describe, match back to a legal move,
## and improvise when no combination applies. Pure functions over the board.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")

static func describe(b, move: Dictionary, color: String) -> String:
	var piece = b.type_of(b.squares[move["from"]])
	var prefix = "" if piece == "P" else piece
	var take := "x" if b.squares[move["to"]] != "" or move.get("en_passant", false) else "-"
	if move.get("castle", "") == "k":
		return "O-O"
	if move.get("castle", "") == "q":
		return "O-O-O"
	var promo: String = ""
	if move.get("promote", "") != "":
		promo = "=" + str(move["promote"])
	return "%s%s%s%s%s" % [prefix, Geo.square_name(move["from"]), take, Geo.square_name(move["to"]), promo]

## Turns a plugin's `{from,to,promote}` back into the engine's own legal move (with the
## correct castle / en-passant flags), rejecting anything not currently legal.
static func match_suggestion(b, picks: Array, color: String) -> Dictionary:
	var legal = b.legal_moves(color)
	for pick in picks:
		for move in legal:
			if move["from"] == pick["from"] and move["to"] == pick["to"]:
				var want := str(pick.get("promote", ""))
				if want == "" or move.get("promote", "") == want:
					return move
	return {}

## No combination applied: grab what you can, else a random legal move, so the
## game always continues.
static func fallback_move(b, color: String) -> Dictionary:
	var best := {}
	var best_gain := -1
	for info in b.annotated_moves(color):
		if info["captured_value"] > best_gain:
			best_gain = info["captured_value"]
			best = info["raw"]
	if best.is_empty():
		return {}
	if best_gain <= 0:
		# Nothing to take — pick any legal move so play stays lively.
		var legal = b.legal_moves(color)
		if not legal.is_empty():
			return legal[randi() % legal.size()]
	return best
