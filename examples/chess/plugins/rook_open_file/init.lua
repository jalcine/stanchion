-- "Open file": put a rook where no pawn stands — files are what rooks are for.
-- Scans the destination file for any pawn of either color.
local RookOpenFile = {}
RookOpenFile.__index = RookOpenFile

function RookOpenFile.new(config, deps)
  return setmetatable({}, RookOpenFile)
end

function RookOpenFile:name() return "Open File" end

local function at(board, sq) return board[sq + 1] end

local function open_file(board, f)
  for r = 0, 7 do
    local code = at(board, r * 8 + f)
    if code == "wP" or code == "bP" then
      return false
    end
  end
  return true
end

function RookOpenFile:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.piece == "R" and open_file(position.board, move.to % 8) then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 240,
        name = "Open File",
        rationale = move.san .. " seizes the open file.",
      }
    end
  end
  return nil
end

return RookOpenFile
