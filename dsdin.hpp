#ifndef DSDIN_HXX_INCLUDED
#define DSDIN_HXX_INCLUDED

#include <algorithm>
#include "dsdin.h"

class dsd
{
    dsd_reader_t *reader;

public:
    dsd(FILE *in_file, FILE *out_file, uint32_t format, uint32_t sample_rate,
        uint8_t channel_count)
    {
        dsd_reader_open(in_file, reader);
    }

    ~dsd()
    {
        dsd_reader_close(reader);
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