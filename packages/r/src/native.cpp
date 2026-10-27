#define R_NO_REMAP
#include <R.h>
#include <Rinternals.h>
#include <Rversion.h>
#include <R_ext/Rdynload.h>
#include <R_ext/Utils.h>

#include <algorithm>
#include <cerrno>
#include <charconv>
#include <climits>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <string>
#include <time.h>
#include <unistd.h>
#include <utility>
#include <vector>

extern "C" size_t mw_connection_read(SEXP source, unsigned char* byte);

namespace {
constexpr double safe_integer = 9007199254740991.0;
constexpr size_t line_limit = 1024 * 1024;

struct Protected {
    SEXP value;
    explicit Protected(SEXP value) : value(PROTECT(value)) {}
    ~Protected() { UNPROTECT(1); }
    Protected(const Protected&) = delete;
    Protected& operator=(const Protected&) = delete;
};

void require(bool valid, const char* message) {
    if (!valid) throw std::runtime_error(message);
}

bool no_attributes(SEXP value) {
#if R_VERSION >= R_Version(4, 6, 0)
    return R_getAttribCount(value) == 0;
#else
    return ATTRIB(value) == R_NilValue;
#endif
}

std::vector<std::string> attribute_names(SEXP value) {
    std::vector<std::string> result;
#if R_VERSION >= R_Version(4, 6, 0)
    Protected names(R_getAttribNames(value));
    for (R_xlen_t i = 0; i < XLENGTH(names.value); ++i)
        result.emplace_back(CHAR(STRING_ELT(names.value, i)));
#else
    for (SEXP attr = ATTRIB(value); attr != R_NilValue; attr = CDR(attr))
        result.emplace_back(CHAR(PRINTNAME(TAG(attr))));
#endif
    return result;
}

std::vector<unsigned> scalars(const std::string& text) {
    std::vector<unsigned> result;
    for (size_t i = 0; i < text.size();) {
        unsigned c = static_cast<unsigned char>(text[i++]);
        unsigned value = c;
        int count = 0;
        if (c >= 0xc2 && c <= 0xdf) { count = 1; value = c & 31; }
        else if (c >= 0xe0 && c <= 0xef) { count = 2; value = c & 15; }
        else if (c >= 0xf0 && c <= 0xf4) { count = 3; value = c & 7; }
        else require(c < 0x80, "Invalid UTF-8.");
        for (int j = 0; j < count; ++j) {
            require(i < text.size(), "Truncated UTF-8.");
            c = static_cast<unsigned char>(text[i++]);
            require((c & 0xc0) == 0x80, "Invalid UTF-8 continuation.");
            value = (value << 6) | (c & 63);
        }
        require((count != 1 || value >= 0x80) && (count != 2 || value >= 0x800) &&
                (count != 3 || value >= 0x10000) && value <= 0x10ffff &&
                !(value >= 0xd800 && value <= 0xdfff), "Invalid Unicode scalar value.");
        result.push_back(value);
    }
    return result;
}

std::vector<unsigned> utf16(const std::string& text) {
    std::vector<unsigned> result;
    for (unsigned c : scalars(text)) {
        if (c > 0xffff) {
            result.push_back(0xd800 + ((c - 0x10000) >> 10));
            result.push_back(0xdc00 + ((c - 0x10000) & 1023));
        } else result.push_back(c);
    }
    return result;
}

void append_scalar(std::string& out, unsigned c) {
    if (c < 0x80) out += static_cast<char>(c);
    else if (c < 0x800) {
        out += static_cast<char>(0xc0 | (c >> 6));
        out += static_cast<char>(0x80 | (c & 63));
    } else if (c < 0x10000) {
        out += static_cast<char>(0xe0 | (c >> 12));
        out += static_cast<char>(0x80 | ((c >> 6) & 63));
        out += static_cast<char>(0x80 | (c & 63));
    } else {
        out += static_cast<char>(0xf0 | (c >> 18));
        out += static_cast<char>(0x80 | ((c >> 12) & 63));
        out += static_cast<char>(0x80 | ((c >> 6) & 63));
        out += static_cast<char>(0x80 | (c & 63));
    }
}

std::string string_value(SEXP value) {
    std::string result;
    if (TYPEOF(value) == RAWSXP && Rf_inherits(value, "metewand_string")) {
        require(Rf_length(Rf_getAttrib(value, R_ClassSymbol)) == 1,
                "Unsupported string class.");
        for (const auto& attr : attribute_names(value))
            require(attr == "class", "Unsupported JSON string attribute.");
        result.assign(reinterpret_cast<const char*>(RAW(value)), XLENGTH(value));
    } else {
        require(TYPEOF(value) == STRSXP && XLENGTH(value) == 1 &&
                no_attributes(value) && STRING_ELT(value, 0) != NA_STRING,
                "JSON strings must be character scalars or json_string values.");
        require(Rf_getCharCE(STRING_ELT(value, 0)) != CE_BYTES,
                "Byte-encoded R strings must be converted explicitly.");
        result = Rf_translateCharUTF8(STRING_ELT(value, 0));
    }
    scalars(result);
    return result;
}

std::string quote(const std::string& value) {
    scalars(value);
    const char* hex = "0123456789abcdef";
    std::string out = "\"";
    for (unsigned char c : value) {
        switch (c) {
        case '"': out += "\\\""; break;
        case '\\': out += "\\\\"; break;
        case '\b': out += "\\b"; break;
        case '\t': out += "\\t"; break;
        case '\n': out += "\\n"; break;
        case '\f': out += "\\f"; break;
        case '\r': out += "\\r"; break;
        default:
            if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
            else out += static_cast<char>(c);
        }
    }
    return out + '"';
}

void check_number(double value) {
    require(std::isfinite(value), "Numbers must be finite binary64 values.");
    require(std::trunc(value) != value || std::abs(value) <= safe_integer,
            "Integral numbers must lie in the 53-bit safe range.");
}

std::string number(double value) {
    check_number(value);
    if (value == 0) return "0";
    char buffer[64];
    // Scientific mode obtains the shortest significant digits independently of
    // the library's fixed-notation threshold. RFC 8785 sets that threshold.
    auto converted = std::to_chars(buffer, buffer + sizeof(buffer), value,
                                   std::chars_format::scientific);
    require(converted.ec == std::errc(), "Binary64 formatting failed.");
    std::string text(buffer, converted.ptr);
    size_t e = text.find('e');
    int exponent = std::stoi(text.substr(e + 1));
    bool negative = value < 0;
    std::string digits = text.substr(negative ? 1 : 0, e - (negative ? 1 : 0));
    digits.erase(std::remove(digits.begin(), digits.end(), '.'), digits.end());
    std::string sign = negative ? "-" : "";
    if (exponent < -6) {
        std::string mantissa(1, digits[0]);
        if (digits.size() > 1) mantissa += "." + digits.substr(1);
        return sign + mantissa + "e" + std::to_string(exponent);
    }
    int point = exponent + 1;
    if (point <= 0) return sign + "0." + std::string(-point, '0') + digits;
    if (point >= static_cast<int>(digits.size()))
        return sign + digits + std::string(point - digits.size(), '0');
    return sign + digits.substr(0, point) + "." + digits.substr(point);
}

SEXP raw_value(const std::string& value) {
    SEXP result = Rf_allocVector(RAWSXP, value.size());
    std::memcpy(RAW(result), value.data(), value.size());
    return result;
}

SEXP r_string(const std::string& value) {
    if (value.find('\0') == std::string::npos) {
        return Rf_ScalarString(Rf_mkCharLenCE(value.data(), value.size(), CE_UTF8));
    }
    Protected result(raw_value(value));
    Protected cls(Rf_mkString("metewand_string"));
    Rf_setAttrib(result.value, R_ClassSymbol, cls.value);
    return result.value;
}

std::string encode(SEXP value, std::vector<SEXP>& ancestors) {
    require(ancestors.size() < 128, "JSON nesting exceeds 128 levels.");
    if (value == R_NilValue) return "null";
    if (TYPEOF(value) == STRSXP || Rf_inherits(value, "metewand_string"))
        return quote(string_value(value));
    if (TYPEOF(value) == VECSXP) {
        require(std::find(ancestors.begin(), ancestors.end(), value) == ancestors.end(),
                "JSON containers cannot contain cycles.");
        bool wrapped = Rf_inherits(value, "metewand_object");
        require(!Rf_isObject(value) || wrapped, "Unsupported JSON object class.");
        if (wrapped) require(Rf_length(Rf_getAttrib(value, R_ClassSymbol)) == 1,
                             "Unsupported object class.");
        for (const auto& attr : attribute_names(value)) {
            require(attr == "names" || (wrapped && (attr == "class" || attr == "keys")),
                    "Unsupported JSON list attribute.");
        }
        SEXP names = Rf_getAttrib(value, R_NamesSymbol);
        SEXP keys = Rf_getAttrib(value, Rf_install("keys"));
        bool object = names != R_NilValue || wrapped;
        std::vector<std::pair<std::vector<unsigned>, R_xlen_t>> order;
        std::vector<std::string> strings;
        if (object) {
            require(wrapped ? TYPEOF(keys) == VECSXP && XLENGTH(keys) == XLENGTH(value)
                            : TYPEOF(names) == STRSXP && XLENGTH(names) == XLENGTH(value),
                    "Objects require one string key per value.");
            for (R_xlen_t i = 0; i < XLENGTH(value); ++i) {
                Protected key(wrapped ? VECTOR_ELT(keys, i) : Rf_ScalarString(STRING_ELT(names, i)));
                strings.push_back(string_value(key.value));
                order.emplace_back(utf16(strings.back()), i);
            }
            std::sort(order.begin(), order.end());
            for (size_t i = 1; i < order.size(); ++i)
                require(order[i-1].first != order[i].first, "Duplicate JSON object key.");
        } else {
            for (R_xlen_t i = 0; i < XLENGTH(value); ++i) order.emplace_back(std::vector<unsigned>(), i);
        }
        ancestors.push_back(value);
        std::string out = object ? "{" : "[";
        bool first = true;
        for (const auto& item : order) {
            if (!first) out += ',';
            first = false;
            if (object) out += quote(strings[item.second]) + ':';
            out += encode(VECTOR_ELT(value, item.second), ancestors);
        }
        ancestors.pop_back();
        return out + (object ? '}' : ']');
    }
    require(TYPEOF(value) == LGLSXP || TYPEOF(value) == INTSXP || TYPEOF(value) == REALSXP,
            "Unsupported JSON value type.");
    require(no_attributes(value) && XLENGTH(value) == 1,
            "JSON scalars must have length one and no attributes. Convert vectors explicitly.");
    if (TYPEOF(value) == LGLSXP) {
        require(LOGICAL(value)[0] != NA_LOGICAL, "NA is not a JSON value.");
        return LOGICAL(value)[0] ? "true" : "false";
    }
    if (TYPEOF(value) == INTSXP) {
        require(INTEGER(value)[0] != NA_INTEGER, "NA is not a JSON value.");
        return number(INTEGER(value)[0]);
    }
    if (TYPEOF(value) == REALSXP) return number(REAL(value)[0]);
    throw std::runtime_error("Unsupported JSON value type.");
}

class Parser {
    const std::string source;
    size_t pos = 0;
    void space() { while (pos < source.size() && std::strchr(" \t\r\n", source[pos]) && source[pos]) ++pos; }
    bool take(char c) { if (pos < source.size() && source[pos] == c) { ++pos; return true; } return false; }
    void expect(char c) { require(take(c), "Malformed JSON."); }
    unsigned hex4() {
        unsigned value = 0;
        for (int i = 0; i < 4; ++i) {
            require(pos < source.size(), "Truncated Unicode escape.");
            char c = source[pos++];
            int digit = c >= '0' && c <= '9' ? c - '0' :
                        c >= 'a' && c <= 'f' ? c - 'a' + 10 :
                        c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;
            require(digit >= 0, "Invalid Unicode escape.");
            value = value * 16 + digit;
        }
        return value;
    }
    std::string string() {
        expect('"');
        std::string out;
        while (pos < source.size()) {
            unsigned char c = source[pos++];
            if (c == '"') { scalars(out); return out; }
            require(c >= 32, "Unescaped control character.");
            if (c != '\\') { out += static_cast<char>(c); continue; }
            require(pos < source.size(), "Truncated string escape.");
            switch (source[pos++]) {
            case '"': out += '"'; break;
            case '\\': out += '\\'; break;
            case '/': out += '/'; break;
            case 'b': out += '\b'; break;
            case 'f': out += '\f'; break;
            case 'n': out += '\n'; break;
            case 'r': out += '\r'; break;
            case 't': out += '\t'; break;
            case 'u': {
                unsigned scalar = hex4();
                if (scalar >= 0xd800 && scalar <= 0xdbff) {
                    expect('\\'); expect('u');
                    unsigned low = hex4();
                    require(low >= 0xdc00 && low <= 0xdfff, "Invalid surrogate pair.");
                    scalar = 0x10000 + ((scalar - 0xd800) << 10) + low - 0xdc00;
                } else require(!(scalar >= 0xdc00 && scalar <= 0xdfff), "Lone Unicode surrogate.");
                append_scalar(out, scalar);
                break;
            }
            default: throw std::runtime_error("Invalid string escape.");
            }
        }
        throw std::runtime_error("Unterminated JSON string.");
    }
    bool digit() const { return pos < source.size() && source[pos] >= '0' && source[pos] <= '9'; }
    SEXP numeric() {
        size_t start = pos;
        take('-');
        if (!take('0')) { require(digit() && source[pos] != '0', "Invalid JSON number."); while (digit()) ++pos; }
        if (take('.')) { require(digit(), "Invalid decimal."); while (digit()) ++pos; }
        if (take('e') || take('E')) {
            if (!take('+')) take('-');
            require(digit(), "Invalid exponent."); while (digit()) ++pos;
        }
        std::string token = source.substr(start, pos - start);
        char* end = nullptr;
        // R_strtod always uses a period and correctly rounds subnormal values.
        double value = R_strtod(token.c_str(), &end);
        require(end == token.c_str() + token.size(), "Invalid JSON number.");
        check_number(value);
        return Rf_ScalarReal(value == 0 ? 0 : value);
    }
    SEXP value(size_t depth) {
        require(depth < 128, "JSON nesting exceeds 128 levels.");
        space();
        require(pos < source.size(), "Missing JSON value.");
        char c = source[pos];
        if (c == '"') return r_string(string());
        if (c == '[' || c == '{') {
            ++pos;
            bool object = c == '{';
            char close = object ? '}' : ']';
            std::vector<std::string> keys;
            // A protected growable pairlist roots values while nested parsing allocates.
            Protected head(Rf_cons(R_NilValue, R_NilValue));
            SEXP tail = head.value;
            size_t count = 0;
            space();
            if (!take(close)) {
                do {
                    space();
                    if (object) { keys.push_back(string()); space(); expect(':'); }
                    Protected item(value(depth + 1));
                    SEXP cell = Rf_cons(item.value, R_NilValue);
                    SETCDR(tail, cell); tail = cell;
                    ++count;
                    space();
                    if (take(close)) break;
                    expect(',');
                } while (true);
            }
            Protected result(Rf_allocVector(VECSXP, count));
            tail = CDR(head.value);
            for (size_t i = 0; i < count; ++i, tail = CDR(tail)) SET_VECTOR_ELT(result.value, i, CAR(tail));
            if (object) {
                auto sorted = keys;
                std::sort(sorted.begin(), sorted.end());
                require(std::adjacent_find(sorted.begin(), sorted.end()) == sorted.end(), "Duplicate JSON object key.");
                bool nul = std::any_of(keys.begin(), keys.end(), [](const std::string& k) { return k.find('\0') != std::string::npos; });
                Protected names(Rf_allocVector(nul ? VECSXP : STRSXP, count));
                for (size_t i = 0; i < count; ++i) {
                    if (nul) SET_VECTOR_ELT(names.value, i, r_string(keys[i]));
                    else SET_STRING_ELT(names.value, i, Rf_mkCharLenCE(keys[i].data(), keys[i].size(), CE_UTF8));
                }
                if (nul) {
                    Protected cls(Rf_mkString("metewand_object"));
                    Rf_setAttrib(result.value, R_ClassSymbol, cls.value);
                    Rf_setAttrib(result.value, Rf_install("keys"), names.value);
                    Protected lookup(Rf_allocVector(STRSXP, count));
                    for (size_t i = 0; i < count; ++i) {
                        const std::string key = keys[i].find('\0') == std::string::npos ? keys[i] : "";
                        SET_STRING_ELT(lookup.value, i, Rf_mkCharLenCE(key.data(), key.size(), CE_UTF8));
                    }
                    Rf_setAttrib(result.value, R_NamesSymbol, lookup.value);
                } else Rf_setAttrib(result.value, R_NamesSymbol, names.value);
            }
            return result.value;
        }
        for (const auto& literal : {std::string("null"), std::string("true"), std::string("false")}) {
            if (source.compare(pos, literal.size(), literal) == 0) {
                pos += literal.size();
                return literal == "null" ? R_NilValue : Rf_ScalarLogical(literal == "true");
            }
        }
        return numeric();
    }
public:
    explicit Parser(std::string source) : source(std::move(source)) {}
    SEXP parse() {
        scalars(source);
        Protected result(value(0));
        space();
        require(pos == source.size(), "Trailing JSON data.");
        return result.value;
    }
};

template <class F> SEXP guarded(F fn) {
    try {
        Protected result(fn());
        SEXP envelope = Rf_allocVector(VECSXP, 1);
        SET_VECTOR_ELT(envelope, 0, result.value);
        return envelope;
    } catch (const std::exception& error) { return Rf_mkString(error.what()); }
}

struct Pipes {
    FILE* reader = nullptr;
    FILE* writer = nullptr;
    ~Pipes() { if (reader) fclose(reader); if (writer) fclose(writer); }
};
void close_pipes(SEXP pointer) {
    delete static_cast<Pipes*>(R_ExternalPtrAddr(pointer));
    R_ClearExternalPtr(pointer);
}
Pipes* pipes(SEXP pointer) {
    require(TYPEOF(pointer) == EXTPTRSXP && R_ExternalPtrAddr(pointer), "Protocol pipes are closed.");
    return static_cast<Pipes*>(R_ExternalPtrAddr(pointer));
}
int descriptor(SEXP value) {
    double fd = Rf_asReal(value);
    require(std::isfinite(fd) && fd >= 0 && fd <= INT_MAX && fd == std::trunc(fd), "Invalid protocol descriptor.");
    return static_cast<int>(fd);
}
}

extern "C" SEXP mw_encode(SEXP value) {
    return guarded([&]() { std::vector<SEXP> ancestors; return raw_value(encode(value, ancestors)); });
}
extern "C" SEXP mw_parse(SEXP source) {
    return guarded([&]() { return Parser(std::string(reinterpret_cast<const char*>(RAW(source)), XLENGTH(source))).parse(); });
}
extern "C" SEXP mw_open(SEXP read_fd, SEXP write_fd) {
    return guarded([&]() {
        int input = descriptor(read_fd), output = descriptor(write_fd);
        require(input != output, "The protocol requires separate descriptors.");
        Protected pointer(R_MakeExternalPtr(nullptr, R_NilValue, R_NilValue));
        R_RegisterCFinalizerEx(pointer.value, close_pipes, TRUE);
        auto pair = std::make_unique<Pipes>();
        int fd = dup(input);
        require(fd >= 0, "Could not duplicate protocol read descriptor.");
        pair->reader = fdopen(fd, "rb");
        if (!pair->reader) { close(fd); throw std::runtime_error("Could not open protocol reader."); }
        fd = dup(output);
        require(fd >= 0, "Could not duplicate protocol write descriptor.");
        pair->writer = fdopen(fd, "wb");
        if (!pair->writer) { close(fd); throw std::runtime_error("Could not open protocol writer."); }
        R_SetExternalPtrAddr(pointer.value, pair.release());
        return pointer.value;
    });
}
extern "C" SEXP mw_close(SEXP pointer) { close_pipes(pointer); return R_NilValue; }
extern "C" SEXP mw_read(SEXP source) {
    return guarded([&]() {
        FILE* file = TYPEOF(source) == EXTPTRSXP ? pipes(source)->reader : nullptr;
        std::string line;
        for (size_t i = 0; i <= line_limit; ++i) {
            unsigned char byte;
            if (file) {
                int c = fgetc(file);
                require(c != EOF, "Protocol stream ended before a complete frame.");
                byte = static_cast<unsigned char>(c);
            } else require(mw_connection_read(source, &byte) == 1, "Protocol stream ended before a complete frame.");
            line += static_cast<char>(byte);
            require(line.size() <= line_limit, "Protocol line exceeds the 1 MiB limit.");
            if (byte == '\n') return raw_value(line);
        }
        throw std::runtime_error("Protocol line exceeds the 1 MiB limit.");
    });
}
extern "C" SEXP mw_write(SEXP target, SEXP bytes) {
    return guarded([&]() {
        FILE* file = pipes(target)->writer;
        size_t length = XLENGTH(bytes);
        require(fwrite(RAW(bytes), 1, length, file) == length && fflush(file) == 0,
                "Protocol response could not be written.");
        return R_NilValue;
    });
}
extern "C" SEXP mw_clock() {
    return guarded([]() {
        timespec now;
        require(clock_gettime(CLOCK_MONOTONIC, &now) == 0, "Monotonic clock is unavailable.");
        return Rf_ScalarReal(now.tv_sec * 1e9 + now.tv_nsec);
    });
}

extern "C" void R_init_metewand(DllInfo* dll) {
    static const R_CallMethodDef methods[] = {
        {"C_mw_encode", reinterpret_cast<DL_FUNC>(&mw_encode), 1},
        {"C_mw_parse", reinterpret_cast<DL_FUNC>(&mw_parse), 1},
        {"C_mw_open", reinterpret_cast<DL_FUNC>(&mw_open), 2},
        {"C_mw_close", reinterpret_cast<DL_FUNC>(&mw_close), 1},
        {"C_mw_read", reinterpret_cast<DL_FUNC>(&mw_read), 1},
        {"C_mw_write", reinterpret_cast<DL_FUNC>(&mw_write), 2},
        {"C_mw_clock", reinterpret_cast<DL_FUNC>(&mw_clock), 0},
        {nullptr, nullptr, 0}
    };
    R_registerRoutines(dll, nullptr, methods, nullptr, nullptr);
    R_useDynamicSymbols(dll, FALSE);
    R_forceSymbols(dll, TRUE);
}
