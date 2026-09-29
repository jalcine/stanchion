-- "Anastasia's Mate": rook and knight mate a king trapped on an edge file (but
-- not the corner — that is the Arabian's). Rook seals the file, knight covers
-- the escape.
local AnastasiaMate = {}
AnastasiaMate.__index = AnastasiaMate

function AnastasiaMate.new(config, deps)
  return setmetatable({}, AnastasiaMate)
end

function AnastasiaMate:name() return "Anastasia's Mate" end

local function at(board, sq) return board[sq + 1] end

function AnastasiaMate:suggest(position)
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
  if not ((f == 0 or f == 7) and r >= 1 and r <= 6) then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.is_mate and (move.piece == "R" or move.piece == "N") then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Anastasia's Mate",
        rationale = move.san .. " is Anastasia's — rook on the file, knight on the flight.",
      }
    end
  end
  return nil
end

return AnastasiaMate
