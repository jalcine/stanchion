-- "Removal of the defender": capture a guarded piece and find the square safe
-- afterwards — the guard was the defender, and it just got traded off. Reads
-- captured_defended (pre-move guard) + attackers_of_to (post-move guard).
local RemovalOfDefender = {}
RemovalOfDefender.__index = RemovalOfDefender

function RemovalOfDefender.new(config, deps)
  return setmetatable({}, RemovalOfDefender)
end

function RemovalOfDefender:name() return "Removal of the Defender" end

function RemovalOfDefender:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    local left = move.attackers_of_to or {}
    if move.captured_defended and #left == 0 and move.captured_value >= 3 then
      if best == nil or move.captured_value > best.captured_value then
        best = move
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.from,
    to = best.to,
    promote = best.promotes,
    strength = 540 + best.captured_value * 5,
    name = "Removal of the Defender",
    rationale = best.san .. " removes the defender — the square goes quiet.",
  }
end

return RemovalOfDefender
