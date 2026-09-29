-- "En passant": the rarest capture on the board — and it often opens the very
-- line the position was about. Take it when it is there.
local EnPassantTheme = {}
EnPassantTheme.__index = EnPassantTheme

function EnPassantTheme.new(config, deps)
  return setmetatable({}, EnPassantTheme)
end

function EnPassantTheme:name() return "En Passant" end

function EnPassantTheme:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.is_en_passant then
      return {
        from = move.from,
        to = move.to,
        promote = "",
        strength = 350,
        name = "En Passant",
        rationale = move.san .. " takes en passant while it is legal.",
      }
    end
  end
  return nil
end

return EnPassantTheme
