-- "Double check": two pieces check the king on one move. Neither check can be
-- blocked or captured away, so the king must move — usually into worse.
-- Reads the engine's is_double_check flag.
local DoubleCheck = {}
DoubleCheck.__index = DoubleCheck

function DoubleCheck.new(config, deps)
  return setmetatable({}, DoubleCheck)
end

function DoubleCheck:name() return "Double Check" end

function DoubleCheck:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.is_double_check and not move.is_mate then
      local gain = move.captured_value + move.attacks_valuable * 2
      if best == nil or gain > best.gain then
        best = { move = move, gain = gain }
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.move.from,
    to = best.move.to,
    promote = best.move.promotes,
    strength = 700 + best.gain * 5,
    name = "Double Check",
    rationale = best.move.san .. " is double check — the king must run.",
  }
end

return DoubleCheck
