-- "Desperado": a piece is attacked and likely lost — spend it loudly. Move the
-- doomed piece with a capture or a check so it takes something down with it.
-- Reads the foe_attacks map for attacked own squares.
local Desperado = {}
Desperado.__index = Desperado

function Desperado.new(config, deps)
  return setmetatable({}, Desperado)
end

function Desperado:name() return "Desperado" end

function Desperado:suggest(position)
  local fa = position.foe_attacks or {}
  local best = nil
  for _, move in ipairs(position.moves) do
    local hit = fa[move.from] or {}
    if #hit >= 1 and (move.captured_value > 0 or move.gives_check) then
      local gain = move.captured_value + (move.gives_check and 2 or 0)
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
    strength = 420 + best.gain * 5,
    name = "Desperado",
    rationale = best.move.san .. " spends the doomed piece with interest.",
  }
end

return Desperado
