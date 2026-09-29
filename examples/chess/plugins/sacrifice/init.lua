-- "Sacrifice": give the exchange (or more) for tempo — the piece goes in cheap
-- and the check or the new attack pays for it. Only with a forcing follow-up;
-- naked blunders are the fallback's job, not a combination's.
local Sacrifice = {}
Sacrifice.__index = Sacrifice

function Sacrifice.new(config, deps)
  return setmetatable({}, Sacrifice)
end

function Sacrifice:name() return "Sacrifice" end

function Sacrifice:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    local cost = (move.mover_value or 0) - move.captured_value
    if (move.mover_value or 0) >= 3 and cost >= 2
        and (move.gives_check or move.attacks_valuable >= 1) then
      local gain = move.attacks_valuable * 2 + move.captured_value
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
    strength = 530 + best.gain * 5,
    name = "Sacrifice",
    rationale = best.move.san .. " gives material for a forcing attack.",
  }
end

return Sacrifice
