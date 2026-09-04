# Builds the Coracle CDT trace plugin against an installed QEMU's plugin header.
#
# macOS: header ships in the Homebrew qemu bottle; plugins are .dylib and
# resolve qemu_plugin_* lazily against the host binary.
# Linux: header ships in qemu-system-common / the QEMU source tree.

UNAME_S := $(shell uname -s)

GLIB_CFLAGS := $(shell pkg-config --cflags glib-2.0)

ifeq ($(UNAME_S),Darwin)
  QEMU_PREFIX ?= $(shell brew --prefix qemu)
  SOEXT := dylib
  LDFLAGS_EXTRA := -undefined dynamic_lookup
else
  QEMU_PREFIX ?= /usr
  SOEXT := so
  LDFLAGS_EXTRA :=
endif

QEMU_INCLUDE ?= $(QEMU_PREFIX)/include

TARGET := libcoracle_trace.$(SOEXT)
SOURCES := coracle_trace.c regmap.c trace_writer.c
HEADERS := cdt_format.h regmap.h trace_writer.h

CFLAGS := -O2 -std=gnu11 -Wall -Wextra -Wno-unused-parameter \
          -fPIC -fvisibility=hidden -I$(QEMU_INCLUDE) $(GLIB_CFLAGS)

.PHONY: all clean
all: $(TARGET)

$(TARGET): $(SOURCES) $(HEADERS)
	$(CC) $(CFLAGS) -shared -o $@ $(SOURCES) $(LDFLAGS_EXTRA)

clean:
	rm -f $(TARGET)
