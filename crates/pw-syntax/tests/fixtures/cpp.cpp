// A comment.
#include <string>
#include <vector>

namespace pitwall {

template <typename T>
class Box final {
public:
    explicit Box(T value) : value_(std::move(value)) {}
    auto get() const noexcept -> const T& { return value_; }

private:
    T value_;
};

}  // namespace pitwall

int main() {
    auto names = std::vector<std::string>{"a\n", R"(raw)"};
    pitwall::Box<int> box{42};
    return box.get() > 0 ? 0 : 1;
}
