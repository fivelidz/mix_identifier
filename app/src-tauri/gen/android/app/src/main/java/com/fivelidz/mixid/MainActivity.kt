package com.fivelidz.mixid

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    requestAudioPermission()
  }

  /**
   * MixID reads audio files by direct file path (Rust std::fs under the hood),
   * which on Android 13+ needs READ_MEDIA_AUDIO granted at runtime
   * (and READ_EXTERNAL_STORAGE on older versions). Without the runtime grant
   * the manifest permission alone is not enough.
   */
  private fun requestAudioPermission() {
    val permission = if (Build.VERSION.SDK_INT >= 33) {
      Manifest.permission.READ_MEDIA_AUDIO
    } else {
      Manifest.permission.READ_EXTERNAL_STORAGE
    }
    if (ContextCompat.checkSelfPermission(this, permission) != PackageManager.PERMISSION_GRANTED) {
      ActivityCompat.requestPermissions(this, arrayOf(permission), 1001)
    }
  }
}
