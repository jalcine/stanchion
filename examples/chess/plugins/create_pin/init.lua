-- "Pin": land a bishop, rook or queen so an enemy piece is stuck in front of a more
-- valuable one (or its king) and cannot move without losing it. Godot computes the
-- geometry and flags `creates_pin` with the `tactic_value` at stake.
local CreatePin = {}
CreatePin.__index = CreatePin

function CreatePin.new(config, deps)
  return setmetatable({}, CreatePin)
end

function CreatePin:name() return "Pin" end

function CreatePin:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    -- Skip a pin that leaves our own piece hanging on the pinning square.
    if move.creates_pin and not move.to_is_attacked then
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
    strength = 180 + best.tactic_value * 15,
    name = "Pin",
    rationale = best.san .. " pins an enemy piece against a bigger one.",
  }
end

return CreatePin
