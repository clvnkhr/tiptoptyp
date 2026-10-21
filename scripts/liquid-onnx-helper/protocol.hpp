#pragma once

#include <charconv>
#include <cstdint>
#include <istream>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace liquid::protocol {

inline constexpr std::size_t kMaximumLineBytes = 4096;
inline constexpr std::size_t kMaximumTokens = 128;
inline constexpr std::int64_t kVocabularySize = 64400;
inline constexpr std::int64_t kTagCount = 128802;

// Retain a bounded prefix, but always consume the complete malformed line so
// that the next request starts at its own request ID.
inline bool read_line(std::istream& input, std::string& line, bool& too_long) {
  line.clear();
  too_long = false;
  char character;
  bool received = false;
  while (input.get(character)) {
    received = true;
    if (character == '\n') {
      return true;
    }
    if (line.size() < kMaximumLineBytes) {
      line.push_back(character);
    } else {
      too_long = true;
    }
  }
  return received;
}

// ONNX may retain a symbolic output batch dimension derived from fixed batch1.
// Actual inference responses still require the exact requested [1, sequence].
inline bool valid_model_shape(const std::vector<std::int64_t>& shape, bool input) {
  return shape.size() == 2 && shape[1] == -1 &&
         (shape[0] == 1 || (!input && shape[0] == -1));
}

inline bool is_space(char character) {
  return character == ' ' || character == '\t' || character == '\r' ||
         character == '\n' || character == '\f' || character == '\v';
}

inline std::string_view next_word(std::string_view line, std::size_t& offset) {
  while (offset < line.size() && is_space(line[offset])) {
    ++offset;
  }
  const auto start = offset;
  while (offset < line.size() && !is_space(line[offset])) {
    ++offset;
  }
  return line.substr(start, offset - start);
}

template <class Integer>
inline bool parse_integer(std::string_view word, Integer& output) {
  if (word.empty()) {
    return false;
  }
  const auto result =
      std::from_chars(word.data(), word.data() + word.size(), output);
  return result.ec == std::errc() && result.ptr == word.data() + word.size();
}

enum class Error { None, LineTooLong, InvalidId, InvalidTokens };

struct Request {
  std::optional<std::uint64_t> id;
  std::vector<std::int64_t> tokens;
  Error error = Error::None;
};

inline Request parse_request(std::string_view line, bool too_long) {
  Request request;
  std::size_t offset = 0;
  std::uint64_t identifier;
  if (!parse_integer(next_word(line, offset), identifier)) {
    request.error = Error::InvalidId;
    return request;
  }
  request.id = identifier;
  if (too_long) {
    request.error = Error::LineTooLong;
    return request;
  }
  request.tokens.reserve(kMaximumTokens);
  while (true) {
    const auto word = next_word(line, offset);
    if (word.empty()) {
      break;
    }
    std::int64_t token;
    if (request.tokens.size() == kMaximumTokens ||
        !parse_integer(word, token) || token < 0 || token >= kVocabularySize) {
      request.error = Error::InvalidTokens;
      return request;
    }
    request.tokens.push_back(token);
  }
  if (request.tokens.empty()) {
    request.error = Error::InvalidTokens;
  }
  return request;
}

inline const char* message(Error error) {
  switch (error) {
    case Error::LineTooLong:
      return "Liquid request is too long.";
    case Error::InvalidId:
      return "Invalid Liquid request ID.";
    case Error::InvalidTokens:
      return "Invalid Liquid token sequence.";
    case Error::None:
      return "Invalid Liquid request.";
  }
  return "Invalid Liquid request.";
}

}  // namespace liquid::protocol
