-- "Boden's Mate": two bishops criss-cross on the long diagonals and mate a king
-- walled in by its own pieces. Requires both own bishops still on the board.
local BodensMate = {}
BodensMate.__index = BodensMate

function BodensMate.new(config, deps)
  return setmetatable({}, BodensMate)
end

function BodensMate:name() return "Boden's Mate" end

local function at(board, sq) return board[sq + 1] end

function BodensMate:suggest(position)
  local bishops = 0
  for sq = 0, 63 do
    if at(position.board, sq) == position.side .. "B" then
      bishops = bishops + 1
    end
  end
  if bishops < 2 then return nil end
  for _, move in ipairs(position.moves) do
    if move.is_mate and move.piece == "B" then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Boden's Mate",
        rationale = move.san .. " is Boden's — the bishops criss-cross over the walled king.",
      }
    end
  end
  return nil
end

return BodensMate
