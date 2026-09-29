## Side panel: title, status, difficulty row, action buttons, AI reasoning,
## player hints, move log. Structure lives in main.tscn; this keeps label
## references and the difficulty-button sync. Button presses connect straight
## to the game in the scene — no relay signals here.
extends VBoxContainer

@onready var status_label: Label = %StatusLabel
@onready var hint_label: RichTextLabel = %HintLabel
@onready var ai_label: RichTextLabel = %AILabel
@onready var log_label: RichTextLabel = %LogLabel


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
