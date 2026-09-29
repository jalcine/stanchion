-- "Outpost": plant a knight where no enemy pawn can ever chase it, and where it
-- already bites material. Knights on outposts win games by standing still.
local OutpostKnight = {}
OutpostKnight.__index = OutpostKnight

function OutpostKnight.new(config, deps)
  return setmetatable({}, OutpostKnight)
end

function OutpostKnight:name() return "Outpost" end

local function at(board, sq) return board[sq + 1] end

-- Can any foe pawn ever attack (f, r)? Foe pawns attack one step forward.
local function pawn_safe(board, f, r, side)
  local back = r - 1
  local pawn = "wP"
  if side == "b" then
    back = r + 1
    pawn = "bP"
  end
  for _, nf in ipairs({f - 1, f + 1}) do
    if nf >= 0 and nf < 8 and back >= 0 and back < 8 then
      if at(board, back * 8 + nf) == pawn then
        return false
      end
    end
  end
  return true
end

function OutpostKnight:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.piece == "N" and move.attacks_valuable >= 1 then
      local f = move.to % 8
      local r = math.floor(move.to / 8)
      if pawn_safe(position.board, f, r, position.side) then
        return {
          from = move.from,
          to = move.to,
          promote = move.promotes,
          strength = 300,
          name = "Outpost",
          rationale = move.san .. " plants an unchaseable outpost.",
        }
      end
    end
  end
  return nil
end

return OutpostKnight
