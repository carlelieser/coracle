#include "trace_writer.h"

#include <stdlib.h>
#include <string.h>

static void flush(struct trace_writer *writer)
{
    if (writer->used == 0) {
        return;
    }
    size_t written = fwrite(writer->buffer, 1, writer->used, writer->file);
    if (written != writer->used) {
        fprintf(stderr, "coracle-trace: short write of %zu bytes\n",
                writer->used);
        abort();
    }
    writer->used = 0;
}

bool trace_writer_open(struct trace_writer *writer, const char *path,
                       const struct cdt_file_header *header)
{
    writer->file = fopen(path, "wb");
    if (!writer->file) {
        fprintf(stderr, "coracle-trace: cannot open trace file '%s'\n", path);
        return false;
    }
    writer->used = 0;
    if (fwrite(header, sizeof(*header), 1, writer->file) != 1) {
        fprintf(stderr, "coracle-trace: cannot write header to '%s'\n", path);
        fclose(writer->file);
        writer->file = NULL;
        return false;
    }
    return true;
}

void trace_writer_emit(struct trace_writer *writer, const void *record,
                       size_t length)
{
    if (!writer->file) {
        return;
    }
    if (writer->used + length > TRACE_WRITER_BUFFER) {
        flush(writer);
    }
    if (length > TRACE_WRITER_BUFFER) {
        if (fwrite(record, 1, length, writer->file) != length) {
            fprintf(stderr, "coracle-trace: short write of %zu bytes\n", length);
            abort();
        }
        return;
    }
    memcpy(writer->buffer + writer->used, record, length);
    writer->used += length;
}

void trace_writer_close(struct trace_writer *writer)
{
    if (!writer->file) {
        return;
    }
    flush(writer);
    fclose(writer->file);
    writer->file = NULL;
}
