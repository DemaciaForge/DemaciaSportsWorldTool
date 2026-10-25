package org.demaciasportsworld.tool;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import java.io.File;
import java.io.FileNotFoundException;

/**
 * 只读提供私有目录里的 update.apk，供系统安装器经 content URI 读取。
 * 不引 androidx，按需最小实现其余 ContentProvider 抽象方法。
 */
public final class ApkProvider extends ContentProvider {
    public static final String AUTHORITY = "org.demaciasportsworld.tool.apk";
    public static final String FILE_NAME = "update.apk";

    public static Uri uriForUpdate() {
        return new Uri.Builder()
                .scheme("content")
                .authority(AUTHORITY)
                .appendPath(FILE_NAME)
                .build();
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!FILE_NAME.equals(uri.getLastPathSegment()) || (mode != null && !mode.startsWith("r"))) {
            throw new FileNotFoundException("Unsupported update uri: " + uri);
        }
        File file = new File(getContext().getFilesDir(), FILE_NAME);
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection,
                        String[] selectionArgs, String sortOrder) {
        return null;
    }

    @Override
    public String getType(Uri uri) {
        return "application/vnd.android.package-archive";
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        return 0;
    }
}
