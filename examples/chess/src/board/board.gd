## The rules of chess, in Godot — facade over focused components.
##
## State lives here (squares, castling, en passant, history). Movement comes from
## pieces/* (one class per piece family), attack questions from attacks.gd,
## notation from san.gd, and the plugin contract from annotate.gd.
## Squares are indexed 0..63 as `rank * 8 + file`, with rank 0 the white back rank
## (a1..h1) and file 0 the a-file. Pieces are two-character codes like "wP" or "bK";
## an empty square is the empty string.
extends RefCounted

const Geo = preload("res://src/board/geometry.gd")
const Movegen = preload("res://src/board/pieces/movegen.gd")
const Attacks = preload("res://src/board/attacks.gd")
const San = preload("res://src/board/san.gd")
const Annotate = preload("res://src/board/annotate.gd")

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

## One raw move dict. Piece classes build through this so the shape stays uniform.
func add_move(moves: Array, from: int, to: int, extra: Dictionary = {}) -> void:
	var move := {"from": from, "to": to, "promote": "", "castle": "", "en_passant": false}
	move.merge(extra, true)
	moves.append(move)

## Every legal move for `color`, each a Dictionary with `from`, `to` and flags. Moves
## that would leave the mover's own king in check are filtered out.
func legal_moves(color: String) -> Array:
	var legal: Array = []
	for move in Movegen.pseudo(self, color):
		var trial = clone()
		trial._make(move)
		if not trial.in_check(color):
			legal.append(move)
	return legal

func king_square(color: String) -> int:
	var target := color + "K"
	for sq in 64:
		if squares[sq] == target:
			return sq
	return -1

func in_check(color: String) -> bool:
	var ks := king_square(color)
	return ks != -1 and is_attacked(ks, opponent(color))

# ---- thin delegates (dynamic calls from clones and components land here) ----

func color_of(code: String) -> String:
	return Geo.color_of(code)

func type_of(code: String) -> String:
	return Geo.type_of(code)

func opponent(color: String) -> String:
	return Geo.opponent(color)

## Whether `by_color` attacks `sq` (used for check and castling-through-check).
func is_attacked(sq: int, by_color: String) -> bool:
	return Attacks.is_attacked(squares, sq, by_color)

func _attackers_of(sq: int, by_color: String) -> Array:
	return Attacks.attackers_of(squares, sq, by_color)

func attacked_targets(sq: int) -> Array:
	return Annotate.attacked_targets(self, sq)

func material(color: String) -> int:
	return Annotate.material(self, color)

func annotated_moves(color: String) -> Array:
	return Annotate.annotated_moves(self, color)

func position_for(color: String) -> Dictionary:
	return Annotate.position_for(self, color)

# ---- applying moves ---------------------------------------------------------

## Applies a move to this board (used both on the live board and on trial clones).
func _make(move: Dictionary) -> void:
	var from: int = move["from"]
	var to: int = move["to"]
	var code := squares[from]
	var color := Geo.color_of(code)
	var rank := Geo.rank_of(from)

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
	if Geo.type_of(code) == "P" and abs(Geo.rank_of(to) - rank) == 2:
		en_passant = (from + to) / 2

	_revoke_castling(from, to, color)
	if color == "b":
		fullmove += 1
	side_to_move = Geo.opponent(color)

func _revoke_castling(from: int, to: int, color: String) -> void:
	if Geo.type_of(squares[to]) == "K" or (from == king_square(color) and squares[to] == color + "K"):
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
	history_san.append(San.san(self, move, legal_moves(side_to_move)))
	_make(move)
