-- "X-ray attack": a slider pins and hits through — the piece in front is already
-- dead, the ray just hasn't collected yet. Reads creates_pin + attacks on
-- valuable material behind the pin.
local XRayAttack = {}
XRayAttack.__index = XRayAttack

function XRayAttack.new(config, deps)
  return setmetatable({}, XRayAttack)
end

function XRayAttack:name() return "X-Ray Attack" end

function XRayAttack:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if (move.piece == "R" or move.piece == "B" or move.piece == "Q")
        and move.creates_pin and move.attacks_valuable >= 1 then
      if best == nil or move.attacks_valuable > best.attacks_valuable then
        best = move
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.from,
    to = best.to,
    promote = best.promotes,
    strength = 490 + best.attacks_valuable * 10,
    name = "X-Ray Attack",
    rationale = best.san .. " sees through the pin to the material behind.",
  }
end

return XRayAttack
