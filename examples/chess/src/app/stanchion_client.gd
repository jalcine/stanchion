## Stanchion host wiring: open the extension, load/reload plugins, dispatch
## suggest calls. Returns data; the game turns it into notes and labels.
extends RefCounted

var host = null  # the GDExtension Stanchion object, or null when missing

## Opens the plugin host. Returns "" on success, else the note to show.
func open_engine(plugins_path: String):
	if not ClassDB.class_exists("Stanchion"):
		return "Stanchion extension not built — see the README. Running rules only, no AI plays."
	host = ClassDB.instantiate("Stanchion")
	# No capabilities, tight-ish limits: these plugins only read a table and return one.
	var ok: bool = host.open({"plugins": plugins_path})
	if not ok:
		var err := "Stanchion.open failed: " + str(host.get_last_error())
		host = null
		return err
	return ""

func is_live() -> bool:
	return host != null

func load_all() -> Array:
	if host == null:
		return []
	var report: Dictionary = host.load("")
	return report.get("loaded", [])

func plugin_names() -> Array:
	if host == null:
		return []
	return host.names()

func reload_plugin(name: String) -> bool:
	if host == null:
		return false
	return host.reload(name)

## Asks every loaded combination for its move, returning suggestions for the
## `allowed` plugins only, strongest first.
func suggest(position: Dictionary, allowed: Array) -> Array:
	if host == null:
		return []
	var outcomes: Array = host.dispatch("suggest", [position])
	var picks: Array = []
	for outcome in outcomes:
		var plugin_name: String = outcome.get("plugin", "")
		if not allowed.has(plugin_name):
			continue
		var value = outcome.get("value")
		if value is Dictionary and value.has("from"):
			value["plugin"] = plugin_name
			picks.append(value)
	picks.sort_custom(func(a, b): return int(a.get("strength", 0)) > int(b.get("strength", 0)))
	return picks
