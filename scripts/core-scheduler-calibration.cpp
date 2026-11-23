#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <evntrace.h>
#include <evntcons.h>
#include <objbase.h>

#include <algorithm>
#include <cstddef>
#include <cstring>
#include <cwchar>
#include <iostream>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace {
constexpr GUID threadProvider{0x3d6fa8d1, 0xfe05, 0x11d0,
                             {0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c}};
constexpr unsigned trialCount = 24;
constexpr size_t switchLimit = 250000;

void check(ULONG status, const char* operation) {
    if (status != ERROR_SUCCESS)
        throw std::runtime_error(std::string(operation) + ": " + std::to_string(status));
}

bool hostedRunner() {
    wchar_t actions[16]{};
    wchar_t runner[32]{};
    GetEnvironmentVariableW(L"GITHUB_ACTIONS", actions, ARRAYSIZE(actions));
    GetEnvironmentVariableW(L"RUNNER_ENVIRONMENT", runner, ARRAYSIZE(runner));
    return wcscmp(actions, L"true") == 0 && wcscmp(runner, L"github-hosted") == 0;
}

struct TraceProperties {
    EVENT_TRACE_PROPERTIES value{};
    wchar_t name[1024]{};
    wchar_t file[1024]{};

    TraceProperties() {
        value.Wnode.BufferSize = sizeof(*this);
        value.LoggerNameOffset = offsetof(TraceProperties, name);
        value.LogFileNameOffset = offsetof(TraceProperties, file);
    }
};

struct TraceSession {
    TRACEHANDLE handle{};
    GUID guid{};

    explicit TraceSession(const wchar_t* path) {
        if (FAILED(CoCreateGuid(&guid))) throw std::runtime_error("CoCreateGuid failed");
        wchar_t name[64]{};
        if (!StringFromGUID2(guid, name, ARRAYSIZE(name)))
            throw std::runtime_error("Cannot name trace session");

        TraceProperties p;
        p.value.Wnode.Guid = guid;
        p.value.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.value.Wnode.ClientContext = 1; // QPC
        p.value.EnableFlags = EVENT_TRACE_FLAG_THREAD | EVENT_TRACE_FLAG_CSWITCH;
        p.value.LogFileMode = EVENT_TRACE_SYSTEM_LOGGER_MODE | EVENT_TRACE_FILE_MODE_SEQUENTIAL;
        p.value.MaximumFileSize = 16;
        p.value.BufferSize = 64;
        p.value.MinimumBuffers = 32;
        p.value.MaximumBuffers = 64;
        if (wcscpy_s(p.file, path) != 0) throw std::runtime_error("Trace path too long");
        check(StartTraceW(&handle, name, &p.value), "StartTraceW");
    }

    TraceSession(const TraceSession&) = delete;
    TraceSession& operator=(const TraceSession&) = delete;

    ULONG stop(TraceProperties& p) noexcept {
        p.value.Wnode.Guid = guid;
        const ULONG status = ControlTraceW(handle, nullptr, &p.value, EVENT_TRACE_CONTROL_STOP);
        if (status == ERROR_SUCCESS) handle = 0;
        return status;
    }

    ~TraceSession() {
        if (handle) {
            TraceProperties p;
            stop(p);
        }
    }
};

LONGLONG qpc() {
    LARGE_INTEGER now{};
    QueryPerformanceCounter(&now);
    return now.QuadPart;
}

struct Span {
    LONGLONG start;
    LONGLONG end;
    unsigned index;
    bool sleep;
};

struct Switch {
    LONGLONG time;
    unsigned cpu;
    ULONG from;
    ULONG to;
};

struct TraceRows {
    DWORD pid = GetCurrentProcessId();
    DWORD tid = 0;
    LONGLONG start = 0;
    LONGLONG end = 0;
    bool invalid = false;
    std::vector<Switch> switches;
};

void WINAPI readEvent(EVENT_RECORD* event) {
    auto& rows = *static_cast<TraceRows*>(event->UserContext);
    auto& h = event->EventHeader;
    if (rows.invalid || !IsEqualGUID(h.ProviderId, threadProvider)) return;
    const auto opcode = h.EventDescriptor.Opcode;
    if (opcode != 36 && opcode != 1 && opcode != 2) return;
    if (event->UserDataLength < 2 * sizeof(ULONG)) {
        rows.invalid = true;
        return;
    }
    // CSwitch begins with NewThreadId and OldThreadId. Thread start/end
    // begin with ProcessId and TThreadId. All four documented fields are u32.
    ULONG fields[2]{};
    std::memcpy(fields, event->UserData, sizeof(fields));
    const auto time = h.TimeStamp.QuadPart;
    if (opcode == 36) {
        if (rows.switches.size() == switchLimit) {
            rows.invalid = true;
            return;
        }
        const unsigned cpu = (h.Flags & EVENT_HEADER_FLAG_PROCESSOR_INDEX)
            ? event->BufferContext.ProcessorIndex : event->BufferContext.ProcessorNumber;
        rows.switches.push_back({time, cpu, fields[1], fields[0]});
    } else if (fields[1] == rows.tid) {
        auto& mark = opcode == 1 ? rows.start : rows.end;
        if (mark || fields[0] != rows.pid) rows.invalid = true;
        mark = time;
    }
}
} // namespace

int wmain(int argc, wchar_t** argv) {
    if (!hostedRunner() || argc != 2) {
        std::cerr << "Synthetic scheduler calibration requires a GitHub-hosted Windows runner\n";
        return 2;
    }
    try {
        LARGE_INTEGER frequency{};
        if (!QueryPerformanceFrequency(&frequency))
            throw std::runtime_error("QueryPerformanceFrequency failed");

        // No raw event file is sent off the disposable runner.
        std::vector<Span> spans;
        spans.reserve(trialCount * 2);
        TraceRows rows;
        rows.switches.reserve(switchLimit);
        TraceProperties p;
        TraceSession session(argv[1]);
        std::thread worker([&] {
            rows.tid = GetCurrentThreadId();
            for (unsigned i = 0; i < trialCount; ++i) {
                const auto start = qpc();
                const auto target = start + frequency.QuadPart * (100 + (i % 3) * 200) / 1000000;
                LONGLONG end;
                do { end = qpc(); } while (end < target);
                spans.push_back({start, end, i, false});

                const auto wait = qpc();
                Sleep(15);
                spans.push_back({wait, qpc(), i, true});
            }
        });
        worker.join();
        check(session.stop(p), "ControlTraceW(STOP)");

        EVENT_TRACE_LOGFILEW log{};
        log.LogFileName = argv[1];
        log.ProcessTraceMode = PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        log.EventRecordCallback = readEvent;
        log.Context = &rows;
        auto consumer = OpenTraceW(&log);
        if (consumer == INVALID_PROCESSTRACE_HANDLE) check(GetLastError(), "OpenTraceW");
        const auto status = ProcessTrace(&consumer, 1, nullptr, nullptr);
        const auto close = CloseTrace(consumer);
        check(status, "ProcessTrace");
        check(close, "CloseTrace");
        if (rows.invalid || !rows.start || rows.start >= rows.end
            || log.LogfileHeader.PerfFreq.QuadPart != frequency.QuadPart)
            throw std::runtime_error("Ambiguous or incomplete scheduler evidence");

        std::stable_sort(rows.switches.begin(), rows.switches.end(),
                         [](const Switch& lhs, const Switch& rhs) { return lhs.time < rhs.time; });
        // Use a shared origin to keep integer ticks exactly representable in JS.
        const auto origin = rows.switches.empty() ? rows.start
            : (std::min)(rows.start, rows.switches.front().time);
        std::cout << "trace " << frequency.QuadPart << ' ' << p.value.EventsLost
                  << ' ' << log.LogfileHeader.EventsLost << ' ' << p.value.LogBuffersLost
                  << ' ' << log.LogfileHeader.BuffersLost << ' ' << p.value.RealTimeBuffersLost << '\n';
        std::cout << "thread " << rows.tid << ' ' << rows.start - origin << ' '
                  << rows.end - origin << '\n';
        for (const auto& row : rows.switches)
            std::cout << "switch " << row.time - origin << ' ' << row.cpu << ' '
                      << row.from << ' ' << row.to << '\n';
        for (const auto& span : spans)
            std::cout << (span.sleep ? "sleep " : "spin ") << span.index << ' '
                      << span.start - origin << ' ' << span.end - origin << '\n';
        return std::cout ? 0 : 1;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
