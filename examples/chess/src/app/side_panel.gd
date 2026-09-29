## Side panel: title, status, difficulty row, action buttons, AI reasoning,
## player hints, move log. Emits intent; the game decides.
extends VBoxContainer

signal difficulty_chosen(name: String)
signal restart_requested
signal reload_requested

var status_label: Label
var hint_label: RichTextLabel
var ai_label: RichTextLabel
var log_label: RichTextLabel


func setup(board_px: float, difficulty: String, roster_names: Array) -> void:
	position = Vector2(board_px + 48, 24)
	custom_minimum_size = Vector2(340, board_px)

	var title := Label.new()
	title.text = "Stanchion Chess"
	title.add_theme_font_size_override("font_size", 26)
	add_child(title)

	status_label = Label.new()
	status_label.add_theme_font_size_override("font_size", 18)
	add_child(status_label)

	var diff_row := HBoxContainer.new()
	add_child(diff_row)
	var diff_caption := Label.new()
	diff_caption.text = "Difficulty:"
	diff_row.add_child(diff_caption)
	for roster_name in roster_names:
		var button := Button.new()
		button.text = roster_name
		button.toggle_mode = true
		button.button_pressed = roster_name == difficulty
		button.pressed.connect(_on_difficulty_button.bind(roster_name))
		diff_row.add_child(button)

	var buttons := HBoxContainer.new()
	add_child(buttons)

	var restart := Button.new()
	restart.text = "New game"
	restart.pressed.connect(func(): restart_requested.emit())
	buttons.add_child(restart)

	# Re-reads the combination plugins from disk without restarting the game — edit a
	# tactic's Lua, press this, and the AI plays the new logic on its next move.
	var reload := Button.new()
	reload.text = "Reload combinations"
	reload.pressed.connect(func(): reload_requested.emit())
	buttons.add_child(reload)

	var ai_caption := Label.new()
	ai_caption.text = "\nBlack's reasoning"
	ai_caption.add_theme_font_size_override("font_size", 16)
	add_child(ai_caption)

	ai_label = RichTextLabel.new()
	ai_label.bbcode_enabled = true
	ai_label.fit_content = true
	ai_label.custom_minimum_size = Vector2(340, 130)
	ai_label.text = "[i]Black moves after you do.[/i]"
	add_child(ai_label)

	var hint_caption := Label.new()
	hint_caption.text = "\nCombinations available to you"
	hint_caption.add_theme_font_size_override("font_size", 16)
	add_child(hint_caption)

	hint_label = RichTextLabel.new()
	hint_label.bbcode_enabled = true
	hint_label.fit_content = true
	hint_label.custom_minimum_size = Vector2(340, 200)
	add_child(hint_label)

	var log_caption := Label.new()
	log_caption.text = "\nMoves"
	log_caption.add_theme_font_size_override("font_size", 16)
	add_child(log_caption)

	log_label = RichTextLabel.new()
	log_label.fit_content = true
	log_label.custom_minimum_size = Vector2(340, 160)
	add_child(log_label)


func _on_difficulty_button(roster_name: String) -> void:
	difficulty_chosen.emit(roster_name)


func sync_difficulty(current: String, rosters: Dictionary) -> void:
	for node in _all_buttons(self):
		if rosters.has(node.text):
			node.button_pressed = node.text == current


func _all_buttons(node: Node) -> Array:
	var found: Array = []
	for child in node.get_children():
		if child is Button:
			found.append(child)
		found += _all_buttons(child)
	return found
