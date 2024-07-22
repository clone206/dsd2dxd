/**
* DSD Unpack - https://github.com/michaelburton/dsdunpack
 *
* dsdunpack Copyright (c) 2014 by Michael Burton.
*
* This program is free software; you can redistribute it and/or modify
* it under the terms of the GNU General Public License as published by
* the Free Software Foundation; either version 2 of the License, or
* (at your option) any later version.
*
* This program is distributed in the hope that it will be useful,
* but WITHOUT ANY WARRANTY; without even the implied warranty of
* MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
* GNU General Public License for more details.
*
* You should have received a copy of the GNU General Public License
* along with this program; if not, write to the Free Software
* Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
*
*/

#include <stdlib.h>
#include <stdio.h>
#include <string.h>

#include "dsf.h"
#include "dsdin.h"
#include <sys/types.h>

/* Large file seeking on MSVC using the same syntax as GCC */
#ifdef _MSC_VER
#define fseeko _fseeki64
#define ftello _ftelli64
#define off_t int64_t
#endif


static const uint8_t bit_reverse_table[] = {
    0x00, 0x80, 0x40, 0xc0, 0x20, 0xa0, 0x60, 0xe0, 0x10, 0x90, 0x50, 0xd0, 0x30, 0xb0, 0x70, 0xf0,
    0x08, 0x88, 0x48, 0xc8, 0x28, 0xa8, 0x68, 0xe8, 0x18, 0x98, 0x58, 0xd8, 0x38, 0xb8, 0x78, 0xf8,
    0x04, 0x84, 0x44, 0xc4, 0x24, 0xa4, 0x64, 0xe4, 0x14, 0x94, 0x54, 0xd4, 0x34, 0xb4, 0x74, 0xf4,
    0x0c, 0x8c, 0x4c, 0xcc, 0x2c, 0xac, 0x6c, 0xec, 0x1c, 0x9c, 0x5c, 0xdc, 0x3c, 0xbc, 0x7c, 0xfc,
    0x02, 0x82, 0x42, 0xc2, 0x22, 0xa2, 0x62, 0xe2, 0x12, 0x92, 0x52, 0xd2, 0x32, 0xb2, 0x72, 0xf2,
    0x0a, 0x8a, 0x4a, 0xca, 0x2a, 0xaa, 0x6a, 0xea, 0x1a, 0x9a, 0x5a, 0xda, 0x3a, 0xba, 0x7a, 0xfa,
    0x06, 0x86, 0x46, 0xc6, 0x26, 0xa6, 0x66, 0xe6, 0x16, 0x96, 0x56, 0xd6, 0x36, 0xb6, 0x76, 0xf6,
    0x0e, 0x8e, 0x4e, 0xce, 0x2e, 0xae, 0x6e, 0xee, 0x1e, 0x9e, 0x5e, 0xde, 0x3e, 0xbe, 0x7e, 0xfe,
    0x01, 0x81, 0x41, 0xc1, 0x21, 0xa1, 0x61, 0xe1, 0x11, 0x91, 0x51, 0xd1, 0x31, 0xb1, 0x71, 0xf1,
    0x09, 0x89, 0x49, 0xc9, 0x29, 0xa9, 0x69, 0xe9, 0x19, 0x99, 0x59, 0xd9, 0x39, 0xb9, 0x79, 0xf9,
    0x05, 0x85, 0x45, 0xc5, 0x25, 0xa5, 0x65, 0xe5, 0x15, 0x95, 0x55, 0xd5, 0x35, 0xb5, 0x75, 0xf5,
    0x0d, 0x8d, 0x4d, 0xcd, 0x2d, 0xad, 0x6d, 0xed, 0x1d, 0x9d, 0x5d, 0xdd, 0x3d, 0xbd, 0x7d, 0xfd,
    0x03, 0x83, 0x43, 0xc3, 0x23, 0xa3, 0x63, 0xe3, 0x13, 0x93, 0x53, 0xd3, 0x33, 0xb3, 0x73, 0xf3,
    0x0b, 0x8b, 0x4b, 0xcb, 0x2b, 0xab, 0x6b, 0xeb, 0x1b, 0x9b, 0x5b, 0xdb, 0x3b, 0xbb, 0x7b, 0xfb,
    0x07, 0x87, 0x47, 0xc7, 0x27, 0xa7, 0x67, 0xe7, 0x17, 0x97, 0x57, 0xd7, 0x37, 0xb7, 0x77, 0xf7,
    0x0f, 0x8f, 0x4f, 0xcf, 0x2f, 0xaf, 0x6f, 0xef, 0x1f, 0x9f, 0x5f, 0xdf, 0x3f, 0xbf, 0x7f, 0xff
};


typedef struct dsf_read_context_t {
    uint32_t block_size;
    uint8_t *buffer[6];
    uint8_t *buffer_ptr[6];
    int      current_channel;
    int      is_lsb;
    uint64_t id3_start;
    uint64_t id3_size;
    uint64_t bytes_remain;
} dsf_read_context_t;

static int dsf_read_open(FILE *fp, dsd_reader_t *reader)
{
    dsd_chunk_header_t dsd;
    fmt_chunk_t fmt;
    data_chunk_t data;
    dsf_read_context_t *context;
    int i;

    fread(&dsd, DSD_CHUNK_HEADER_SIZE, 1, fp);
    if (dsd.chunk_id != DSD_MARKER) {
        return 0;
    }
    if (htole64(dsd.chunk_data_size) > DSD_CHUNK_HEADER_SIZE) {
        fseeko(fp, htole64(dsd.chunk_data_size) - DSD_CHUNK_HEADER_SIZE, SEEK_CUR);
    }

    fread(&fmt, FMT_CHUNK_SIZE, 1, fp);
    if (fmt.chunk_id != FMT_MARKER || htole32(fmt.format_id) != FORMAT_ID_DSD) {
        return 0;
    }
    reader->channel_count = htole32(fmt.channel_count) & 0xFF;
    reader->sample_rate = htole32(fmt.sample_frequency);
    reader->data_length = htole64(fmt.sample_count) / 8 * reader->channel_count;
    reader->compressed = 0;
    if (htole64(fmt.chunk_data_size) > FMT_CHUNK_SIZE) {
        fseeko(fp, htole64(fmt.chunk_data_size) - FMT_CHUNK_SIZE, SEEK_CUR);
    }

    fread(&data, DATA_CHUNK_SIZE, 1, fp);
    if (data.chunk_id != DATA_MARKER) {
        return 0;
    }

    context = (dsf_read_context_t*) malloc(sizeof(dsf_read_context_t));
    context->bytes_remain = reader->data_length;
    context->block_size = htole32(fmt.block_size_per_channel);
    context->current_channel = 0;
    for (i = 0; i < reader->channel_count; i++) {
        context->buffer[i] = (uint8_t*)malloc(context->block_size);
        context->buffer_ptr[i] = context->buffer[i] + context->block_size;
    }
    context->is_lsb = htole32(fmt.bits_per_sample) == 1;
    context->id3_start = htole64(dsd.metadata_offset);
    if (context->id3_start > 0) {
        context->id3_size = htole64(dsd.total_file_size) - context->id3_start;
    }

    reader->prvt = context;
    return 1;
}

static size_t dsf_read_samples(char *buf, size_t len, dsd_reader_t *reader)
{
    dsf_read_context_t *context = (dsf_read_context_t*) reader->prvt;
    size_t bytes_read;

    if (len > context->bytes_remain) {
        len = (size_t) context->bytes_remain;
    }

    if (context->block_size) {
        char *dest_buf = buf;
        char *dest_buf_end = dest_buf + len;
        int i = context->current_channel;

        bytes_read = 0;
        if (context->is_lsb) {
            while (dest_buf < dest_buf_end) {
                if (context->buffer_ptr[i] == context->buffer[i] + context->block_size) {
                    if (fread(context->buffer[i], 1, context->block_size, reader->input) != context->block_size) {
                        break;
                    }
                    context->buffer_ptr[i] = context->buffer[i];
                }
                *dest_buf++ = (char) bit_reverse_table[*context->buffer_ptr[i]++];

                if (++i == reader->channel_count) {
                    i = 0;
                }
            }
        } else {
            while (dest_buf < dest_buf_end) {
                if (context->buffer_ptr[i] == context->buffer[i] + context->block_size) {
                    if (fread(context->buffer[i], 1, context->block_size, reader->input) != context->block_size) {
                        break;
                    }
                    context->buffer_ptr[i] = context->buffer[i];
                }
                *dest_buf++ = (char) *context->buffer_ptr[i]++;

                if (++i == reader->channel_count) {
                    i = 0;
                }
            }
        }
        bytes_read = dest_buf - buf;
        context->current_channel = i;
    } else {
        bytes_read = fread(buf, 1, len, reader->input);
    }

    context->bytes_remain -= bytes_read;
    return bytes_read;
}

static void dsf_read_close(dsd_reader_t *reader)
{
    dsf_read_context_t *context = (dsf_read_context_t*) reader->prvt;
    if (context->block_size) {
        int i;
        for (i = 0; i < reader->channel_count; i++) {
            free(context->buffer[i]);
        }
        context->block_size = 0;
    }
}

static void dsf_read_clone(dsd_reader_t *reader, dsd_reader_t *reader2) {
    reader2->prvt = (dsf_read_context_t *)malloc(sizeof(dsf_read_context_t));

    if (reader2->prvt) {
        memcpy(reader2->prvt, reader->prvt, sizeof(dsf_read_context_t));
        
        if (((dsf_read_context_t*)reader->prvt)->block_size) {
            int i;
            for (i = 0; i < reader->channel_count; i++) {
                ((dsf_read_context_t*)reader2->prvt)->buffer[i] 
                    = (uint8_t*)malloc(((dsf_read_context_t*)reader->prvt)
                        ->block_size);
                memcpy(((dsf_read_context_t*)reader2->prvt)->buffer[i],
                    ((dsf_read_context_t*)reader->prvt)->buffer[i],
                    ((dsf_read_context_t*)reader->prvt)->block_size);
            }
        }
    }
}

static uint32_t dsf_read_next_chunk(dsd_reader_t *reader)
{
    dsf_read_context_t *context = (dsf_read_context_t*) reader->prvt;

    if (context->id3_start) {
        fseeko(reader->input, context->id3_start, SEEK_SET);
        dsf_read_close(reader); /* free the per-channel buffers */
        context->id3_start = 0; /* prevent returning the same tag again */
        context->bytes_remain = context->id3_size;
        return MAKE_MARKER('I', 'D', '3', ' ');
    }

    return 0;
}

dsd_reader_funcs_t *dsf_reader_funcs()
{
    static dsd_reader_funcs_t funcs = {
        dsf_read_open,
        dsf_read_samples,
        dsf_read_next_chunk,
        dsf_read_close,
        dsf_read_clone
    };
    return &funcs;
}