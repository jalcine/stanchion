-- "Decoy": offer a valuable piece onto an attacked square that also hits back —
-- the piece is the bait, and whatever takes it walks into the attack. Reads
-- mover_value + attackers_of_to + attacks_valuable.
local Decoy = {}
Decoy.__index = Decoy

function Decoy.new(config, deps)
  return setmetatable({}, Decoy)
end

function Decoy:name() return "Decoy" end

function Decoy:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    local under = move.attackers_of_to or {}
    if (move.mover_value or 0) >= 3 and #under >= 1 and move.attacks_valuable >= 1 then
      local gain = move.attacks_valuable + move.captured_value
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
    strength = 500 + best.gain * 10,
    name = "Decoy",
    rationale = best.move.san .. " lures the target onto a mined square.",
  }
end

return Decoy
