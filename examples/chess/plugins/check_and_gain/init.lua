-- "Check and gain": give check and win something with the same move — a capture,
-- or a new attack on valuable material the opponent has no tempo to save.
-- Forcing moves first, booty second.
local CheckAndGain = {}
CheckAndGain.__index = CheckAndGain

function CheckAndGain.new(config, deps)
  return setmetatable({}, CheckAndGain)
end

function CheckAndGain:name() return "Check and Gain" end

function CheckAndGain:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.gives_check and not move.is_mate and not move.is_double_check
        and (move.captured_value > 0 or move.attacks_valuable >= 1) then
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
    strength = 460 + best.gain * 10,
    name = "Check and Gain",
    rationale = best.move.san .. " checks and pockets material with tempo.",
  }
end

return CheckAndGain
