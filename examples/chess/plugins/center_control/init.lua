-- "Control the centre": the first principle of the opening. Push a pawn to one of the
-- four central squares, or aim a knight at them. Falls silent once the opening is over.
local CenterControl = {}
CenterControl.__index = CenterControl

local CENTER = { d4 = true, e4 = true, d5 = true, e5 = true }
local KNIGHT_POSTS = { f3 = true, c3 = true, f6 = true, c6 = true }

function CenterControl.new(config, deps)
  return setmetatable({ until_move = config.until_move or 8 }, CenterControl)
end

function CenterControl:name() return "Control the Centre" end

function CenterControl:suggest(position)
  if position.fullmove > self.until_move then return nil end
  for _, move in ipairs(position.moves) do
    if move.piece == "P" and CENTER[move.to_sq] then
      return {
        from = move.from,
        to = move.to,
        promote = "",
        strength = 130,
        name = "Control the Centre",
        rationale = "Pawn to " .. move.to_sq .. " stakes a claim in the centre.",
      }
    end
  end
  for _, move in ipairs(position.moves) do
    if move.piece == "N" and KNIGHT_POSTS[move.to_sq] then
      return {
        from = move.from,
        to = move.to,
        promote = "",
        strength = 120,
        name = "Control the Centre",
        rationale = "Knight to " .. move.to_sq .. " eyes the centre.",
      }
    end
  end
  return nil
end

return CenterControl
