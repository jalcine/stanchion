-- "Passed pawn": push a pawn no enemy pawn can stop — no same-file foe pawn ahead
-- and none on the neighbouring files either. Scans position.board by file.
local PassedPawnPush = {}
PassedPawnPush.__index = PassedPawnPush

function PassedPawnPush.new(config, deps)
  return setmetatable({}, PassedPawnPush)
end

function PassedPawnPush:name() return "Passed Pawn" end

local function at(board, sq) return board[sq + 1] end

local function passed(board, to, side)
  local foe_pawn = "bP"
  if side == "b" then foe_pawn = "wP" end
  local dir = 1
  if side == "b" then dir = -1 end
  local f = to % 8
  local r = math.floor(to / 8)
  for df = -1, 1 do
    local nf = f + df
    if nf >= 0 and nf < 8 then
      local nr = r + dir
      while nr >= 0 and nr < 8 do
        if at(board, nr * 8 + nf) == foe_pawn then
          return false
        end
        nr = nr + dir
      end
    end
  end
  return true
end

function PassedPawnPush:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.piece == "P" and move.captured_value == 0
        and passed(position.board, move.to, position.side) then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 300,
        name = "Passed Pawn",
        rationale = move.san .. " — no pawn can stop it now.",
      }
    end
  end
  return nil
end

return PassedPawnPush
