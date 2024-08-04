#ifndef DSDIN_HXX_INCLUDED
#define DSDIN_HXX_INCLUDED

#include <algorithm>
#include "dsdin.h"

class dsd
{
    dsd_reader_t *reader;

    void setInterleaved(uint32_t container_format)
    {
        switch (container_format)
        {
        case DSD_FORMAT_DSDIFF:
            interleaved = true;
            break;
        case DSD_FORMAT_DSF:
            interleaved = false;
            break;
        }
    }

    void setDsdRate(uint32_t sample_rate)
    {
        dsdRate = sample_rate / 44100 / 64;
    }

    void setBlockSize(uint32_t block_size) {
        if (block_size)
        {
            blockSize = block_size;
        }
        else
        {
            blockSize = 0;
        }
    }

public:
    long audioLength;
    off_t audioPos; // Position of beginning of audio in input file
    int channelCount;
    int dsdRate;
    bool interleaved;
    bool isLsb;
    int blockSize;

    dsd(FILE *in_file) : reader(new dsd_reader_t)
    {
        if (!reader || dsd_reader_open(in_file, reader) != 1)
            throw std::runtime_error("Couldn't init reader. Check inputs.");
        audioLength = reader->data_length;
        audioPos = reader->audio_pos;
        channelCount = reader->channel_count;
        isLsb = reader->is_lsb == 1 ? true : false;

        setBlockSize(reader->block_size);
        setDsdRate(reader->sample_rate);
        setInterleaved(reader->container_format);
    }

    dsd(dsd const &x) : reader(dsd_reader_clone(x.reader))
    {
        if (!reader)
            throw std::runtime_error("Couldn't clone reader. Check inputs.");
        audioLength = reader->data_length;
        audioPos = reader->audio_pos;
        channelCount = reader->channel_count;
        isLsb = reader->is_lsb == 1 ? true : false;

        setBlockSize(reader->block_size);
        setDsdRate(reader->sample_rate);
        setInterleaved(reader->container_format);
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

    size_t readerRead(char *buf, size_t len, dsd_reader_t *reader)
    {
        return dsd_reader_read(buf, len, reader);
    }

    uint32_t readerNextChunk()
    {
        return dsd_reader_next_chunk(reader);
    }
};

#endif // DSDIO_HXX_INCLUDED