#ifndef DSDIN_HXX_INCLUDED
#define DSDIN_HXX_INCLUDED

#include <algorithm>
#include "dsdin.h"

class dsd
{
    dsd_reader_t *reader;

public:
    dsd(FILE *in_file) : reader(new dsd_reader_t)
    {
        if (!reader || dsd_reader_open(in_file, reader) != 1)
            throw std::runtime_error("Couldn't init. Check inputs.");
    }

    dsd(dsd const &x) : reader(dsd_reader_clone(x.reader)) {
        if (!reader)
            throw std::runtime_error("Couldn't clone. Check inputs.");
    }

    ~dsd()
    {
        dsd_reader_close(reader);
        delete reader;
    }

    friend void swap(dsd &a, dsd &b)
    {
        std::swap(a.reader, b.reader);
    }

    dsd &operator=(dsd x)
    {
        swap(*this, x);
        return *this;
    }

    size_t readerRead(char *buf, size_t len, dsd_reader_t *reader) {
        return dsd_reader_read(buf, len, reader);
    }

    uint32_t readerNextChunk() {
        return dsd_reader_next_chunk(reader);
    }
};

#endif // DSDIO_HXX_INCLUDED