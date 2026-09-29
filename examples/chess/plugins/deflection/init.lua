-- "Deflection": check while taking a guarded piece, so the defender must answer
-- the check instead of doing its job. The guard that mattered is gone by the
-- time the check is parried. Reads gives_check + captured_defended.
local Deflection = {}
Deflection.__index = Deflection

function Deflection.new(config, deps)
  return setmetatable({}, Deflection)
end

function Deflection:name() return "Deflection" end

function Deflection:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.gives_check and not move.is_mate and move.captured_defended
        and move.captured_value >= 3 then
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
    strength = 520 + best.captured_value * 5,
    name = "Deflection",
    rationale = best.san .. " checks and drags the defender off its post.",
  }
end

return Deflection
