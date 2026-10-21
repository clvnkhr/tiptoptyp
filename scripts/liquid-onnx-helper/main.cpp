#include "protocol.hpp"

#include <onnxruntime_cxx_api.h>

#include <array>
#include <cmath>
#include <iomanip>
#include <iostream>
#include <stdexcept>

namespace {

#if defined(__APPLE__) && defined(__aarch64__)
constexpr const char* kTarget = "aarch64-apple-darwin";
#elif defined(__APPLE__) && defined(__x86_64__)
constexpr const char* kTarget = "x86_64-apple-darwin";
#else
#error "The packaged Liquid helper supports macOS arm64 and x86_64."
#endif

constexpr std::array<const char*, 2> kInputNames{"input_ids", "attention_mask"};
constexpr std::array<const char*, 2> kOutputNames{"best_tags", "error_probs"};

void write_identifier(std::optional<std::uint64_t> identifier) {
  if (identifier) {
    std::cout << *identifier;
  } else {
    std::cout << "null";
  }
}

// Never forward model exceptions, source text, token IDs, or filesystem paths.
void write_error(std::optional<std::uint64_t> identifier, const char* message) {
  std::cout << "{\"id\":";
  write_identifier(identifier);
  std::cout << ",\"ok\":false,\"error\":\"" << message << "\"}" << std::endl;
}

void validate_schema(const Ort::Session& session) {
  if (session.GetInputCount() != 2 || session.GetOutputCount() != 2) {
    throw std::runtime_error("invalid model schema");
  }
  Ort::AllocatorWithDefaultOptions allocator;
  for (std::size_t index = 0; index < 2; ++index) {
    auto input_name = session.GetInputNameAllocated(index, allocator);
    auto output_name = session.GetOutputNameAllocated(index, allocator);
    const auto input_type = session.GetInputTypeInfo(index);
    const auto output_type = session.GetOutputTypeInfo(index);
    const auto input = input_type.GetTensorTypeAndShapeInfo();
    const auto output = output_type.GetTensorTypeAndShapeInfo();
    if (std::string_view(input_name.get()) != kInputNames[index] ||
        std::string_view(output_name.get()) != kOutputNames[index] ||
        input.GetElementType() != ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64 ||
        output.GetElementType() != (index == 0 ? ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64
                                             : ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT) ||
        !liquid::protocol::valid_model_shape(input.GetShape(), true) ||
        !liquid::protocol::valid_model_shape(output.GetShape(), false)) {
      throw std::runtime_error("invalid model schema");
    }
  }
}

void predict(Ort::Session& session, const Ort::MemoryInfo& memory,
             liquid::protocol::Request& request) {
  const auto length = static_cast<std::int64_t>(request.tokens.size());
  const std::array<std::int64_t, 2> shape{1, length};
  std::vector<std::int64_t> mask(request.tokens.size(), 1);
  std::array<Ort::Value, 2> inputs{
      Ort::Value::CreateTensor<std::int64_t>(memory, request.tokens.data(),
          request.tokens.size(), shape.data(), shape.size()),
      Ort::Value::CreateTensor<std::int64_t>(memory, mask.data(), mask.size(),
          shape.data(), shape.size())};
  auto outputs = session.Run(Ort::RunOptions{nullptr}, kInputNames.data(),
      inputs.data(), inputs.size(), kOutputNames.data(), kOutputNames.size());
  if (outputs.size() != 2 || !outputs[0].IsTensor() || !outputs[1].IsTensor()) {
    throw std::runtime_error("invalid prediction");
  }
  const auto tags = outputs[0].GetTensorTypeAndShapeInfo();
  const auto probabilities = outputs[1].GetTensorTypeAndShapeInfo();
  const std::vector<std::int64_t> expected_shape{1, length};
  if (tags.GetElementType() != ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64 ||
      probabilities.GetElementType() != ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT ||
      tags.GetShape() != expected_shape || probabilities.GetShape() != expected_shape) {
    throw std::runtime_error("invalid prediction");
  }
  const auto* tag_data = outputs[0].GetTensorData<std::int64_t>();
  const auto* probability_data = outputs[1].GetTensorData<float>();
  for (std::int64_t i = 0; i < length; ++i) {
    if (tag_data[i] < 0 || tag_data[i] >= liquid::protocol::kTagCount ||
        !std::isfinite(probability_data[i]) || probability_data[i] < 0 ||
        probability_data[i] > 1) {
      throw std::runtime_error("invalid prediction");
    }
  }
  std::cout << "{\"id\":";
  write_identifier(request.id);
  std::cout << ",\"ok\":true,\"best_tags\":[";
  for (std::int64_t i = 0; i < length; ++i) {
    if (i != 0) std::cout << ',';
    std::cout << tag_data[i];
  }
  std::cout << "],\"error_probs\":[";
  for (std::int64_t i = 0; i < length; ++i) {
    if (i != 0) std::cout << ',';
    // Nine significant digits round-trip a float32 without extra payload.
    std::cout << std::setprecision(9) << probability_data[i];
  }
  std::cout << "]}" << std::endl;
}

}  // namespace

int main(int argc, char** argv) {
  std::ios::sync_with_stdio(false);
  std::cin.tie(nullptr);
  int threads;
  if (argc != 3 ||
      !liquid::protocol::parse_integer(std::string_view(argv[2]), threads) ||
      threads < 1 || threads > 8) {
    std::cerr << "Liquid requires a model and a CPU thread count from 1 to 8.\n";
    return 2;
  }
  try {
    Ort::Env environment(ORT_LOGGING_LEVEL_FATAL, "Liquid");
    environment.DisableTelemetryEvents();
    Ort::SessionOptions options;
    options.SetIntraOpNumThreads(threads);
    options.SetInterOpNumThreads(1);
    options.SetExecutionMode(ExecutionMode::ORT_SEQUENTIAL);
    options.SetGraphOptimizationLevel(GraphOptimizationLevel::ORT_ENABLE_ALL);
    // Helpers sleep between user selections rather than stealing editor CPU.
    options.AddConfigEntry("session.intra_op.allow_spinning", "0");
    options.AddConfigEntry("session.inter_op.allow_spinning", "0");
    Ort::Session session(environment, argv[1], options);
    validate_schema(session);
    const auto memory = Ort::MemoryInfo::CreateCpu(OrtArenaAllocator, OrtMemTypeDefault);
    std::cout << "{\"ready\":true,\"protocol\":1,\"target\":\"" << kTarget
              << "\",\"threads\":" << threads << '}' << std::endl;
    std::string line;
    line.reserve(liquid::protocol::kMaximumLineBytes);
    bool too_long;
    while (liquid::protocol::read_line(std::cin, line, too_long)) {
      auto request = liquid::protocol::parse_request(line, too_long);
      if (request.error != liquid::protocol::Error::None) {
        write_error(request.id, liquid::protocol::message(request.error));
        if (!std::cout) return 1;
        continue;
      }
      try {
        predict(session, memory, request);
      } catch (...) {
        write_error(request.id, "Liquid inference failed.");
        return 1;
      }
      if (!std::cout) return 1;
    }
    return std::cin.bad() ? 1 : 0;
  } catch (...) {
    std::cerr << "Liquid model could not load.\n";
    return 1;
  }
}
