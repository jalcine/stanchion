-- "Arabian Mate": rook and knight corner a king on a corner square (a1/h1/a8/h8).
-- Fires on is_mate by R/N against a cornered king; stays silent otherwise.
local ArabianMate = {}
ArabianMate.__index = ArabianMate

function ArabianMate.new(config, deps)
  return setmetatable({}, ArabianMate)
end

function ArabianMate:name() return "Arabian Mate" end

local function at(board, sq) return board[sq + 1] end

function ArabianMate:suggest(position)
  local foe = "b"
  if position.side == "b" then foe = "w" end
  local king = -1
  for sq = 0, 63 do
    if at(position.board, sq) == foe .. "K" then
      king = sq
      break
    end
  end
  if king == -1 then return nil end
  local f = king % 8
  local r = math.floor(king / 8)
  if not ((f == 0 or f == 7) and (r == 0 or r == 7)) then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.is_mate and (move.piece == "R" or move.piece == "N") then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Arabian Mate",
        rationale = move.san .. " is the Arabian — rook and knight sew up the corner.",
      }
    end
  end
  return nil
end

return ArabianMate
