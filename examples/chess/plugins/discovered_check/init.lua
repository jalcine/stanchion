-- "Discovered check": move one piece to unveil a check from another. The move itself is
-- free to do damage — grab material or hit a second target — because the opponent must
-- answer the check first. Godot flags `is_discovered_check` when the side gives check
-- but not with the piece that moved.
local Discovered = {}
Discovered.__index = Discovered

function Discovered.new(config, deps)
  return setmetatable({}, Discovered)
end

function Discovered:name() return "Discovered Check" end

function Discovered:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.is_discovered_check then
      -- Prefer the discovery that also wins the most while checking.
      local gain = move.captured_value
      if move.attacks_valuable >= 1 and gain < 3 then gain = 3 end
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
    strength = 560 + best.gain * 10,
    name = "Discovered Check",
    rationale = best.move.san .. " uncovers a check and strikes at the same time.",
  }
end

return Discovered
