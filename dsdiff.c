/**
* DSD Unpack - https://github.com/michaelburton/dsdunpack
*
* Copyright (c) 2014 by Michael Burton.
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

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "dsdiff.h"
#include "dsdin.h"
#include <pthread.h>

#define off_t int64_t
/* Large file seeking on MSVC using the same syntax as GCC */
#ifdef _MSC_VER
#define fseeko _fseeki64
#define ftello _ftelli64
#define off_t int64_t
#endif


typedef struct dsdiff_read_context_t {
    chunk_header_t  current_chunk;
    uint64_t        bytes_read;
    off_t           next_chunk;

    uint8_t        *fake_id3;
    uint64_t        fake_id3_len;

    uint32_t        dst_frame_size;
    uint32_t        dst_frames_remain;
    char           *dest_buffer;
    uint32_t        dest_buffer_frames_remain;
    pthread_cond_t  dst_decode_done;
    pthread_mutex_t dst_decode_done_mutex;
} dsdiff_read_context_t;

static void dsdiff_dst_decode_done(uint8_t *frame_data, size_t frame_size, void *userdata)
{
    dsdiff_read_context_t *context = (dsdiff_read_context_t*) userdata;

    memcpy(context->dest_buffer, frame_data, frame_size);
    context->dest_buffer += frame_size;

    if (!--context->dest_buffer_frames_remain) {
        pthread_mutex_lock(&context->dst_decode_done_mutex);
        pthread_cond_signal(&context->dst_decode_done);
        pthread_mutex_unlock(&context->dst_decode_done_mutex);
    }
}

static void dsdiff_dst_decode_error(int frame_count, int frame_error_code, const char *frame_error_message, void *userdata)
{
    fprintf(stderr, "DST decoding error %d: %s\n", frame_error_code, frame_error_message);
}

static int dsdiff_read_open(FILE *fp, dsd_reader_t *reader)
{
    uint8_t *fake_id3 = NULL;
    uint64_t fake_id3_len = 0;

    /* Check FRM8 header */
    {
        form_dsd_chunk_t frm8;
        fread(&frm8, FORM_DSD_CHUNK_SIZE, 1, fp);
        if (frm8.chunk_id != FRM8_MARKER || frm8.form_type != DSD_MARKER) {
            return 0;
        }
    }

    /* Check file version chunk */
    {
        format_version_chunk_t fver;
        fread(&fver, FORMAT_VERSION_CHUNK_SIZE, 1, fp);
        if (fver.chunk_id != FVER_MARKER) {
            return 0;
        }
        if (hton64(fver.chunk_data_size) > 4) {
            fseeko(fp, CEIL_ODD_NUMBER(SWAP64(fver.chunk_data_size)) - 4, SEEK_CUR);
        }
    }

    /* Read audio properties */
    {
        property_chunk_t prop;
        char *props, *cur_prop;

        /* Search for PROP chunk with SND properties, which is guaranteed to be before the sound data */
        prop.chunk_id = 0;
        for (;;) {
            fread(&prop, PROPERTY_CHUNK_SIZE, 1, fp);
            SWAP64(prop.chunk_data_size);
            if (prop.chunk_id == PROP_MARKER && prop.property_type == SND_MARKER) {
                break;
            } else if (feof(fp) || prop.chunk_id == DSD_MARKER || prop.chunk_id == DST_MARKER) {
                return 0;
            } else {
                fseeko(fp, CEIL_ODD_NUMBER(prop.chunk_data_size) - 4, SEEK_CUR);
            }
        }

        /* Read and process property sub-chunks */
        props = (char*)malloc((size_t) CEIL_ODD_NUMBER(prop.chunk_data_size) - 4);
        fread(props, (size_t) CEIL_ODD_NUMBER(prop.chunk_data_size) - 4, 1, fp);
        cur_prop = props;
        while (cur_prop < (props + prop.chunk_data_size - 4)) {
            chunk_header_t *prop_head = (chunk_header_t*) cur_prop;
            if (prop_head->chunk_id == FS_MARKER) {
                sample_rate_chunk_t *samp = (sample_rate_chunk_t*) cur_prop;
                reader->sample_rate = hton32(samp->sample_rate);
            } else if (prop_head->chunk_id == CHNL_MARKER) {
                channels_chunk_t *chnl = (channels_chunk_t*) cur_prop;
                reader->channel_count = (uint8_t) hton16(chnl->channel_count);
            } else if (prop_head->chunk_id == MAKE_MARKER('I', 'D', '3', ' ')) {
                /* Some versions of sacd-ripper put ID3 tags in PROP instead of a chunk
                   at the end of the file, so we pretend it's at the end. */
                fake_id3_len = hton64(prop_head->chunk_data_size);
                fake_id3 = (uint8_t*)malloc((size_t) fake_id3_len);
                memcpy(fake_id3, cur_prop + CHUNK_HEADER_SIZE, (size_t) fake_id3_len);
            }
            cur_prop += CEIL_ODD_NUMBER(hton64(prop_head->chunk_data_size)) + CHUNK_HEADER_SIZE;
        }
        free(props);
    }

    /* And finally, prepare to read audio data */
    {
        chunk_header_t audio;
        dsdiff_read_context_t *context = (dsdiff_read_context_t*) malloc(sizeof(dsdiff_read_context_t));

        /* Find where audio data starts */
        for (;;) {
            fread(&audio, CHUNK_HEADER_SIZE, 1, fp);
            SWAP64(audio.chunk_data_size);
            if (audio.chunk_id == DST_MARKER) {
                dst_frame_information_chunk_t frte;
                chunk_header_t dsti;
                off_t start;
                
                fread(&frte, DST_FRAME_INFORMATION_CHUNK_SIZE, 1, fp);
                context->dst_frame_size = reader->sample_rate / hton16(frte.frame_rate) / 8 * reader->channel_count;
                context->dst_frames_remain = hton32(frte.num_frames);
                reader->data_length = (uint64_t) context->dst_frames_remain * context->dst_frame_size;
                reader->compressed = 1;

                context->dst_decode_done = (pthread_cond_t)PTHREAD_COND_INITIALIZER;
                context->dst_decode_done_mutex = (pthread_mutex_t)PTHREAD_MUTEX_INITIALIZER;

                /* The next 'real' chunk is after the DSTI, so find where the DSTI chunk ends... */
                start = ftello(fp);
                fseeko(fp, CEIL_ODD_NUMBER(audio.chunk_data_size - DST_FRAME_INFORMATION_CHUNK_SIZE), SEEK_CUR);
                fread(&dsti, CHUNK_HEADER_SIZE, 1, fp);
                context->next_chunk = ftello(fp) + CEIL_ODD_NUMBER(hton64(dsti.chunk_data_size));
                fseeko(fp, start, SEEK_SET);
                break;
            } else if (audio.chunk_id == DSD_MARKER) {
                reader->data_length = audio.chunk_data_size;
                reader->compressed = 0;
                context->next_chunk = ftello(fp) + CEIL_ODD_NUMBER(audio.chunk_data_size);
                break;
            } else if (feof(fp)) {
                if (fake_id3) {
                    free(fake_id3);
                }
                free(context);
                return 0;
            } else {
                fseeko(fp, CEIL_ODD_NUMBER(audio.chunk_data_size), SEEK_CUR);
            }
        }
        context->current_chunk = audio;
        context->bytes_read = 0;
        context->fake_id3 = fake_id3;
        context->fake_id3_len = fake_id3_len;
        reader->prvt = context;
    }

    return 1;
}

static size_t dsdiff_read_samples(char *buf, size_t len, dsd_reader_t *reader)
{
    dsdiff_read_context_t *context = (dsdiff_read_context_t*) reader->prvt;
    size_t amount = 0;

    if (context->current_chunk.chunk_id == DST_MARKER) {
        uint32_t frame_count = len / context->dst_frame_size;
        chunk_header_t dstf;
        uint32_t i;
        uint8_t *frame = (uint8_t*)malloc(context->dst_frame_size);
        
        if (frame_count > context->dst_frames_remain) {
            frame_count = context->dst_frames_remain;
        }

        context->dest_buffer = buf;
        context->dest_buffer_frames_remain = frame_count;
        context->dst_frames_remain -= frame_count;
        for (i = 0; i < frame_count; i++) {
            /* Read DST frame and add to decoder queue */
            fread(&dstf, DST_FRAME_DATA_CHUNK_SIZE, 1, reader->input);
            SWAP64(dstf.chunk_data_size);
            if (dstf.chunk_id == DSTC_MARKER) {
                /* Ignore CRC chunks */
                fseeko(reader->input, CEIL_ODD_NUMBER(dstf.chunk_data_size), SEEK_CUR);
                continue;
            } else if (dstf.chunk_id != DSTF_MARKER) {
                context->dst_frames_remain = 0;
                context->dest_buffer_frames_remain -= frame_count - i;
                break;
            }
            fread(frame, 1, (size_t) CEIL_ODD_NUMBER(dstf.chunk_data_size), reader->input);
        }
        free(frame);

        if (frame_count > 0) {
            pthread_mutex_lock(&context->dst_decode_done_mutex);
            pthread_cond_wait(&context->dst_decode_done, &context->dst_decode_done_mutex);
            pthread_mutex_unlock(&context->dst_decode_done_mutex);
        }
        amount = context->dest_buffer - buf;
    } else if (context->next_chunk == 0 && context->fake_id3) {
        /* Reached EOF alerady, so read from the 'fake' ID3 chunk */
        uint64_t bytes_remain = context->fake_id3_len - context->bytes_read;
        amount = (len > bytes_remain) ? (size_t) bytes_remain : len;
        memcpy(buf, context->fake_id3 + context->bytes_read, amount);
        context->bytes_read += amount;
    } else {
        uint64_t bytes_remain = context->current_chunk.chunk_data_size - context->bytes_read;
        amount = (len > bytes_remain) ? (size_t) bytes_remain : len;
        amount = fread(buf, 1, amount, reader->input);
        context->bytes_read += amount;
    }

    return amount;
}

static uint32_t dsdiff_read_next_chunk(dsd_reader_t *reader)
{
    dsdiff_read_context_t *context = (dsdiff_read_context_t*) reader->prvt;

    if (context->next_chunk) {
        context->bytes_read = 0;
        fseeko(reader->input, context->next_chunk, SEEK_SET);
        if (fread(&context->current_chunk, CHUNK_HEADER_SIZE, 1, reader->input) == 1) {
            SWAP64(context->current_chunk.chunk_data_size);
            context->next_chunk = ftello(reader->input) + CEIL_ODD_NUMBER(context->current_chunk.chunk_data_size);
        } else {
            context->current_chunk.chunk_id = context->fake_id3 ? MAKE_MARKER('I', 'D', '3', ' ') : 0;
            context->current_chunk.chunk_data_size = 0;
            context->next_chunk = 0;
        }
    } else if (context->fake_id3) {
        context->current_chunk.chunk_id = 0;
        free(context->fake_id3);
        context->fake_id3 = 0;
    }

    return context->current_chunk.chunk_id;
}

static void dsdiff_read_close(dsd_reader_t *reader)
{
    dsdiff_read_context_t *context = (dsdiff_read_context_t*) reader->prvt;

    if (context->fake_id3) {
        free(context->fake_id3);
        context->fake_id3 = NULL;
    }
}

dsd_reader_funcs_t *dsdiff_reader_funcs()
{
    static dsd_reader_funcs_t funcs = {
        dsdiff_read_open,
        dsdiff_read_samples,
        dsdiff_read_next_chunk,
        dsdiff_read_close
    };
    return &funcs;
}