-- "Cross-check": answer a check with a check of your own. The opponent's attack
-- fizzles — they must get out of check instead of following through.
-- Reads position.in_check + gives_check.
local CrossCheck = {}
CrossCheck.__index = CrossCheck

function CrossCheck.new(config, deps)
  return setmetatable({}, CrossCheck)
end

function CrossCheck:name() return "Cross-Check" end

function CrossCheck:suggest(position)
  if not position.in_check then return nil end
  for _, move in ipairs(position.moves) do
    if move.gives_check then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 480,
        name = "Cross-Check",
        rationale = move.san .. " answers check with check.",
      }
    end
  end
  return nil
end

return CrossCheck
