#pragma once

// The C++ half of Verbatim's GUI: the functions Rust calls through the cxx
// bridge (crates/verbatim-gui/src/bridge.rs). The shared structs they take
// are defined by the generated bridge header, which includes this one first,
// so they are only declared here.

#include <cstddef>
#include <cstdint>

namespace verbatim_gui {

struct GuiCore;
struct ShellText;
struct ScreenPoint;
struct SettingsDialog;
struct ListDialog;
struct SynthesizerSwitch;
enum class DialogKind : std::uint8_t;

std::int32_t run_event_loop(const GuiCore& core, const ShellText& text);
void wake_event_loop();

std::size_t frame_handle();
ScreenPoint centre_frame();
void show_frame();
void hide_frame();
bool popup_menu();

void open_settings_dialog(const SettingsDialog& dialog);
void open_list_dialog(const ListDialog& dialog);
std::size_t dialog_handle(DialogKind dialog);
void raise_dialog(DialogKind dialog);
void focus_dialog(DialogKind dialog);
void synthesizer_switched(const SynthesizerSwitch& outcome);

void shut_down();

} // namespace verbatim_gui
