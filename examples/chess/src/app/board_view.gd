## Board surface: geometry, click input, and hand-drawn pieces/highlights.
## Reads game state (board, selection, hints) but never mutates it.
extends TextureRect

signal square_picked(sq: int)

# The board art is 142×142 with a 7px frame, so the playable 8×8 grid is 16px cells
# inset by 7px (7 + 8*16 + 7 = 142). We render the whole texture at BOARD_PX and derive
# the cell size and inset from those same ratios, so pieces land on the drawn squares
# instead of drifting.
const BOARD_PX := 640.0
const TEX_SIZE := 142.0
const TEX_FRAME := 7.0
const TEX_CELL := 16.0
const SCALE := BOARD_PX / TEX_SIZE
const ORIGIN := TEX_FRAME * SCALE
const CELL := TEX_CELL * SCALE

# Draw the expected cell bounding boxes over the board (a debug aid for alignment).
const SHOW_CELL_BORDERS := false
const GLYPH := {"K": "♚", "Q": "♛", "R": "♜", "B": "♝", "N": "♞", "P": "♟"}

# Per-colour glyph nudges, as a fraction of a cell (x: +right, y: +down). White reads a
# touch left and up; black a touch left and down.
const WHITE_NUDGE := Vector2(0.0, 0.0)
const BLACK_NUDGE := Vector2(0.0, 0.0)

const Geo = preload("res://src/board/geometry.gd")

var game = null  # game.gd — untyped to keep the dependency one-directional
var board_font: Font
var overlay: Node2D
var pieces_layer: Node2D


func setup(g) -> void:
	game = g
	texture = load("res://assets/board.png")
	texture_filter = CanvasItem.TEXTURE_FILTER_NEAREST
	stretch_mode = TextureRect.STRETCH_SCALE
	custom_minimum_size = Vector2(BOARD_PX, BOARD_PX)
	size = Vector2(BOARD_PX, BOARD_PX)
	position = Vector2(24, 24)
	mouse_filter = Control.MOUSE_FILTER_STOP
	gui_input.connect(_on_gui_input)
	board_font = ThemeDB.get_fallback_font()

	# Highlights below, pieces on top, both drawn by hand so glyphs land dead-centre in
	# their squares (a Label centres on the font's line box, not the glyph, so chess
	# glyphs drift high and left).
	overlay = Node2D.new()
	overlay.draw.connect(_draw_overlay)
	add_child(overlay)

	pieces_layer = Node2D.new()
	pieces_layer.draw.connect(_draw_pieces)
	add_child(pieces_layer)


func refresh() -> void:
	pieces_layer.queue_redraw()
	overlay.queue_redraw()


func cell_pos(sq: int) -> Vector2:
	return Vector2(ORIGIN + Geo.file_of(sq) * CELL, ORIGIN + (7 - Geo.rank_of(sq)) * CELL)


func square_at(pos: Vector2) -> int:
	var gx := pos.x - ORIGIN
	var gy := pos.y - ORIGIN
	if gx < 0 or gy < 0 or gx >= CELL * 8 or gy >= CELL * 8:
		return -1
	var file := int(gx / CELL)
	var rank := 7 - int(gy / CELL)
	return rank * 8 + file


func _on_gui_input(event: InputEvent) -> void:
	if not (
		event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT
	):
		return
	var sq := square_at(event.position)
	if sq != -1:
		square_picked.emit(sq)


func _draw_overlay() -> void:
	# Squares the current reasoning refers to, tinted behind the pieces: amber for the
	# move Black just played, blue for the player's top suggested combination.
	if game.ai_last_move.has("from"):
		_tint(game.ai_last_move["from"], Color(0.95, 0.6, 0.2, 0.32))
		_tint(game.ai_last_move["to"], Color(0.95, 0.6, 0.2, 0.32))
	if game._human_turn() and game.hint_move.has("from"):
		_tint(game.hint_move["from"], Color(0.3, 0.6, 1.0, 0.28))
		_tint(game.hint_move["to"], Color(0.3, 0.6, 1.0, 0.28))

	# Selected square and its legal destinations.
	if game.selected != -1:
		overlay.draw_rect(
			Rect2(cell_pos(game.selected), Vector2(CELL, CELL)), Color(1, 0.9, 0.3, 0.35)
		)
	for move in game.selected_moves:
		var center: Vector2 = cell_pos(move["to"]) + Vector2(CELL, CELL) / 2
		var is_capture: bool = game.board.squares[move["to"]] != ""
		if is_capture:
			overlay.draw_arc(center, CELL * 0.42, 0, TAU, 32, Color(0.9, 0.3, 0.2, 0.8), 4.0)
		else:
			overlay.draw_circle(center, CELL * 0.16, Color(0.2, 0.7, 0.3, 0.6))
	# The top suggested combination, drawn as an arrow-ish pair of markers.
	if game._human_turn() and game.hint_move.has("from"):
		var a: Vector2 = cell_pos(game.hint_move["from"]) + Vector2(CELL, CELL) / 2
		var b: Vector2 = cell_pos(game.hint_move["to"]) + Vector2(CELL, CELL) / 2
		overlay.draw_line(a, b, Color(0.3, 0.6, 1.0, 0.7), 5.0)
		overlay.draw_circle(b, CELL * 0.12, Color(0.3, 0.6, 1.0, 0.9))


## Fills one square with a translucent colour.
func _tint(sq: int, color: Color) -> void:
	overlay.draw_rect(Rect2(cell_pos(sq), Vector2(CELL, CELL)), color)


## Draws the pieces as font glyphs, each measured and centred in its square. White
## reads light with a dark outline; black the reverse.
func _draw_pieces() -> void:
	if SHOW_CELL_BORDERS:
		for sq in 64:
			pieces_layer.draw_rect(
				Rect2(cell_pos(sq), Vector2(CELL, CELL)), Color(1, 0, 0, 0.7), false, 1.0
			)

	var font_size := int(CELL * 0.82)
	var outline := int(CELL * 0.06)
	for sq in 64:
		var code: String = game.board.squares[sq]
		if code == "":
			continue
		var glyph: String = GLYPH[game.board.type_of(code)]
		var is_white = game.board.color_of(code) == "w"
		var fill := Color(0.98, 0.98, 0.95) if is_white else Color(0.11, 0.11, 0.13)
		var edge := Color(0.1, 0.1, 0.1) if is_white else Color(0.9, 0.9, 0.9)

		# Centre the glyph on the square using its measured size and the font baseline,
		# then nudge per colour — the black and white glyph ink sits slightly
		# differently in the em box, so each needs its own small correction.
		var extent := board_font.get_string_size(glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size)
		var center: Vector2 = cell_pos(sq) + Vector2(CELL, CELL) / 2
		var nudge := WHITE_NUDGE if is_white else BLACK_NUDGE
		var origin := Vector2(
			center.x - extent.x / 2.0 + nudge.x * CELL,
			center.y - extent.y / 2.0 + board_font.get_ascent(font_size) + nudge.y * CELL
		)
		pieces_layer.draw_string_outline(
			board_font, origin, glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size, outline, edge
		)
		pieces_layer.draw_string(
			board_font, origin, glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size, fill
		)
