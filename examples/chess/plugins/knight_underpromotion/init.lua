-- "Knight underpromotion": queen by default — unless the knight forks or checks
-- on arrival. The rare case the auto-queen rule must not swallow.
local KnightUnderpromotion = {}
KnightUnderpromotion.__index = KnightUnderpromotion

function KnightUnderpromotion.new(config, deps)
  return setmetatable({}, KnightUnderpromotion)
end

function KnightUnderpromotion:name() return "Knight Underpromotion" end

function KnightUnderpromotion:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.promotes == "N" and (move.attacks_valuable >= 1 or move.gives_check) then
      return {
        from = move.from,
        to = move.to,
        promote = "N",
        strength = 470,
        name = "Knight Underpromotion",
        rationale = move.san .. " — the knight's fork is worth more than a queen.",
      }
    end
  end
  return nil
end

return KnightUnderpromotion
