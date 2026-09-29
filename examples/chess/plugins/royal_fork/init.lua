-- "Royal fork": one piece checks the king and hits something valuable at the same
-- time. The king must move, the other piece falls. Outranks ordinary forks —
-- a forced win of material, not a two-way guess.
local RoyalFork = {}
RoyalFork.__index = RoyalFork

function RoyalFork.new(config, deps)
  return setmetatable({}, RoyalFork)
end

function RoyalFork:name() return "Royal Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

local function has_king(attacks)
  for _, code in ipairs(attacks) do
    if code == "K" then return true end
  end
  return false
end

function RoyalFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.attacks_valuable >= 1 and has_king(move.attacks) then
      local gain = move.attacks_valuable
      if best == nil or gain > best.gain then
        best = { move = move, gain = gain }
      end
    end
  end
  if best == nil then return nil end
  local hit = {}
  for _, code in ipairs(best.move.attacks) do
    hit[#hit + 1] = NAMES[code] or code
  end
  return {
    from = best.move.from,
    to = best.move.to,
    promote = best.move.promotes,
    strength = 620 + best.gain * 10,
    name = "Royal Fork",
    rationale = best.move.san .. " checks the king and the " .. table.concat(hit, " and ") .. " — one must fall.",
  }
end

return RoyalFork
