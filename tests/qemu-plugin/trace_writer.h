/*
 * Buffered CDT record writer. Not thread-safe; the plan fixes SMP at one vCPU.
 */
#ifndef CORACLE_TRACE_WRITER_H
#define CORACLE_TRACE_WRITER_H

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

#include "cdt_format.h"

#define TRACE_WRITER_BUFFER 262144

struct trace_writer {
    FILE *file;
    uint8_t buffer[TRACE_WRITER_BUFFER];
    size_t used;
};

bool trace_writer_open(struct trace_writer *writer, const char *path,
                       const struct cdt_file_header *header);
void trace_writer_emit(struct trace_writer *writer, const void *record,
                       size_t length);
void trace_writer_close(struct trace_writer *writer);

#endif /* CORACLE_TRACE_WRITER_H */
