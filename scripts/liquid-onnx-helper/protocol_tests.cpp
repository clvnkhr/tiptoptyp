#include "protocol.hpp"

#include <cstdlib>
#include <iostream>
#include <sstream>

namespace {

void require(bool condition, const char* message = "model shape contract failed") {
  if (!condition) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

void check_requests() {
  using liquid::protocol::Error;
  using liquid::protocol::parse_request;
  auto valid = parse_request("18446744073709551615\t1 0 64399\r", false);
  require(valid.error == Error::None && valid.id == UINT64_MAX &&
              valid.tokens == std::vector<std::int64_t>({1, 0, 64399}),
          "valid boundary values were not preserved");
  for (const auto* line : {"", "-1 1", "18446744073709551616 1", "12x 1"}) {
    const auto request = parse_request(line, false);
    require(request.error == Error::InvalidId && !request.id,
            "invalid request ID was accepted");
  }
  for (const auto* line : {"12", "12 -1", "12 64400", "12 one", "12 1x",
                           "12 1 9223372036854775808", "12 1 -9223372036854775809"}) {
    const auto request = parse_request(line, false);
    require(request.error == Error::InvalidTokens && request.id == 12,
            "invalid tokens were accepted or lost their request ID");
  }
  std::string maximum = "8";
  for (std::size_t i = 0; i < liquid::protocol::kMaximumTokens; ++i) {
    maximum += " 1";
  }
  require(parse_request(maximum, false).error == Error::None,
          "128-token request was rejected");
  require(parse_request(maximum + " 1", false).error == Error::InvalidTokens,
          "129-token request was accepted");
}

void check_bounded_lines() {
  using liquid::protocol::Error;
  using liquid::protocol::parse_request;
  using liquid::protocol::read_line;
  const auto limit = liquid::protocol::kMaximumLineBytes;
  std::string line;
  bool too_long;
  std::istringstream input("7 1" + std::string(limit - 3, ' ') + "\n9 1" +
                           std::string(1024 * 1024, ' ') + "\n11 2\r\n13 3");
  require(read_line(input, line, too_long) && !too_long && line.size() == limit &&
              parse_request(line, too_long).error == Error::None,
          "exact line bound was rejected");
  require(read_line(input, line, too_long) && too_long && line.size() == limit,
          "oversized request retained too many bytes");
  auto rejected = parse_request(line, too_long);
  require(rejected.id == 9 && rejected.error == Error::LineTooLong,
          "oversized request did not retain its ID");
  require(read_line(input, line, too_long) && !too_long &&
              parse_request(line, false).id == 11,
          "oversized request was not drained before the next line");
  require(read_line(input, line, too_long) &&
              parse_request(line, false).id == 13,
          "final line without newline was lost");
  require(!read_line(input, line, too_long), "EOF produced another request");
}

}  // namespace

int main() {
  require(liquid::protocol::valid_model_shape({1, -1}, true));
  require(liquid::protocol::valid_model_shape({1, -1}, false));
  require(liquid::protocol::valid_model_shape({-1, -1}, false));
  require(!liquid::protocol::valid_model_shape({-1, -1}, true));
  require(!liquid::protocol::valid_model_shape({2, -1}, false));
  require(!liquid::protocol::valid_model_shape({1, 128}, false));
  require(!liquid::protocol::valid_model_shape({1}, false));
  check_requests();
  check_bounded_lines();
  return 0;
}
