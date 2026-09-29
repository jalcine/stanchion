-- "Skewer": the pin's mirror image. Attack a valuable piece (often the king) along a
-- line so that when it steps aside, the piece behind it falls. Godot flags
-- `creates_skewer` with the `tactic_value` you expect to win behind it.
local Skewer = {}
Skewer.__index = Skewer

function Skewer.new(config, deps)
  return setmetatable({}, Skewer)
end

function Skewer:name() return "Skewer" end

function Skewer:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    -- Skip a skewer that just hangs the skewering piece.
    if move.creates_skewer and not move.to_is_attacked then
      if best == nil or move.tactic_value > best.tactic_value then
        best = move
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.from,
    to = best.to,
    promote = best.promotes,
    strength = 320 + best.tactic_value * 15,
    name = "Skewer",
    rationale = best.san .. " skewers the piece in front to win the one behind.",
  }
end

return Skewer
