-- "Win material": take the most valuable thing on offer. A blunt but effective
-- combination — most beginner games are decided by who grabs the hanging piece.
local WinMaterial = {}
WinMaterial.__index = WinMaterial

function WinMaterial.new(config, deps)
  return setmetatable({ min_value = config.min_value or 1 }, WinMaterial)
end

function WinMaterial:name() return "Win Material" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function WinMaterial:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.captured_value >= self.min_value then
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
    -- The more it wins, the louder the recommendation.
    strength = 300 + best.captured_value * 15,
    name = "Win Material",
    rationale = "Take the " .. (NAMES[best.capture] or "piece") .. " on " .. best.to_sq
      .. " (+" .. best.captured_value .. ").",
  }
end

return WinMaterial
